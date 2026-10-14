//! 事务纪律回归：配置变更与健康写入都不丢事实、不静默覆盖。
//!
//! 这些用例在**真实** `SqliteSettingsUoW` 上跑，断言的是不变式（两条事实都落地、
//! 陈旧写入被拒且只回滚自己的槽位），不是线程交错本身——单连接顺序执行下交错不可复现，
//! 能钉住的是「无论谁先谁后，结果都必须同时成立」。

use haven_application::services::source_registry::SourceHealthRecorder;
use haven_application::wire::{
    SourceHealthDto, SourceRegistrySetRequest, SourceSetCredentialRequest,
};
use haven_domain::credential::CredentialStore;

use super::fixtures::{
    EditOn, EditingCredentialStore, MemoryStore, current_revision, descriptor_of, obs,
    registry_with_health,
};

/// 停用与健康写入并发：两个事实都必须落地——健康写入不得覆盖配置，
/// 配置写入也不得丢掉已经观测到的健康事实。
#[tokio::test]
async fn concurrent_disable_and_health_write_keep_both_facts() {
    let (registry, _repos) = registry_with_health();
    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();
    // 先启用，让并发的那次停用成为一次真实的状态翻转。
    registry
        .set(SourceRegistrySetRequest {
            source_id: "cms10".into(),
            enabled: true,
        })
        .await
        .unwrap();
    let revision = current_revision(&registry, "cms10").await;

    let (observed, disabled) = tokio::join!(
        registry.record_search_observation(obs("cms10", revision, 1_000, false)),
        registry.set(SourceRegistrySetRequest {
            source_id: "cms10".into(),
            enabled: false,
        }),
    );
    observed.unwrap();
    disabled.unwrap();

    let projected = descriptor_of(&registry, "cms10").await;
    assert!(!projected.enabled, "用户刚写入的停用选择必须落地");
    assert!(projected.endpoint_configured, "端点不得被并发写入清掉");
    assert_eq!(
        projected.health,
        SourceHealthDto::Degraded,
        "并发的真实观测也必须落地"
    );
    assert_eq!(
        registry.endpoint("cms10").await.unwrap().as_deref(),
        Some("https://a.example.invalid/api.php")
    );
}

/// 并发的配置变更不得互相覆盖：整包覆盖写会丢掉先到的那一次。
#[tokio::test]
async fn concurrent_config_changes_do_not_lose_updates() {
    let (registry, _repos) = registry_with_health();
    let (enabled, endpoint) = tokio::join!(
        registry.set(SourceRegistrySetRequest {
            source_id: "cms10".into(),
            enabled: true,
        }),
        registry.set_endpoint("cms10", "https://a.example.invalid/api.php"),
    );
    enabled.unwrap();
    endpoint.unwrap();

    let projected = descriptor_of(&registry, "cms10").await;
    assert!(projected.enabled, "并发写入的启用选择不得被覆盖");
    assert!(projected.endpoint_configured, "并发写入的端点不得被覆盖");
    assert_eq!(
        registry.endpoint("cms10").await.unwrap().as_deref(),
        Some("https://a.example.invalid/api.php")
    );
}

/// 凭据写入采纳前来源已被改写：本次写入作废，且**只删除它自己刚分配的那个版本槽**。
///
/// 旧实现会把「IO 之前读到的旧秘密」还原回去（那可能盖掉赢家的秘密）；新契约里没有
/// 任何还原分支：旧凭据的归属没有因为这次失败的写入而改变。
#[tokio::test]
async fn stale_credential_write_is_rejected_and_only_its_own_slot_is_removed() {
    let (registry, _repos) = registry_with_health();
    let added = registry
        .add_custom_source("我的书源", "https://one.example.invalid/opds")
        .await
        .unwrap();
    let store = EditingCredentialStore::new(
        registry.clone(),
        &added.source_id,
        "https://two.example.invalid/opds",
        EditOn::Set,
    );

    let error = registry
        .set_custom_source_credential(
            &SourceSetCredentialRequest {
                username: None,
                source_id: added.source_id.clone(),
                secret: Some("pw".into()),
            },
            &store,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SOURCE_CONFIG_STALE");

    let written = store.writes();
    assert_eq!(written.len(), 1, "失败的写入只写了一处外部存储");
    assert_eq!(
        store.deletes(),
        written,
        "作废的写入只能删除自己刚写进去的那一个槽位"
    );
    let own_slot = written[0].parse().expect("写入的 target 必须是合法引用");
    assert!(
        store.get(&own_slot).await.unwrap().is_none(),
        "作废的槽位必须被补偿删除，不能留在 keystore 里"
    );
    assert!(
        registry
            .custom_credential_ref(&added.source_id)
            .await
            .unwrap()
            .is_none(),
        "复核失败不得把凭据引用写回去"
    );
    assert_eq!(
        registry
            .endpoint(&added.source_id)
            .await
            .unwrap()
            .as_deref(),
        Some("https://two.example.invalid/opds"),
        "配置写入方的端点与版本必须保持不动"
    );
}

/// 清除期间来源被改配置：撤销在同一个事务里完成、外部删除照常执行；并发的端点写入落地。
///
/// 清除路径不再依赖「IO 之后的复核」，也因此不存在把旧秘密还原回去的分支——
/// 那条分支正是旧实现可能把赢家的秘密盖掉的地方。
#[tokio::test]
async fn clear_during_a_config_edit_deletes_exactly_the_revoked_slot() {
    let (registry, _repos) = registry_with_health();
    let added = registry
        .add_custom_source("我的书源", "https://one.example.invalid/opds")
        .await
        .unwrap();
    registry
        .set_custom_source_credential(
            &SourceSetCredentialRequest {
                username: None,
                source_id: added.source_id.clone(),
                secret: Some("pw".into()),
            },
            &MemoryStore::new(),
        )
        .await
        .unwrap();
    let revoked = registry
        .custom_credential_ref(&added.source_id)
        .await
        .unwrap()
        .expect("写入后必须存在凭据引用");

    // 外部删除期间来源被改地址（配置版本推进）。
    let store = EditingCredentialStore::new(
        registry.clone(),
        &added.source_id,
        "https://two.example.invalid/opds",
        EditOn::Delete,
    );
    store.seed_secret(revoked.as_str(), "pw");

    registry
        .set_custom_source_credential(
            &SourceSetCredentialRequest {
                username: None,
                source_id: added.source_id.clone(),
                secret: None,
            },
            &store,
        )
        .await
        .unwrap();

    assert_eq!(
        store.deletes(),
        vec![revoked.as_str().to_owned()],
        "只删除被撤销的那一个槽位"
    );
    assert!(store.get(&revoked).await.unwrap().is_none());
    assert!(
        registry
            .custom_credential_ref(&added.source_id)
            .await
            .unwrap()
            .is_none(),
        "撤销是权威事实"
    );
    assert_eq!(
        registry
            .endpoint(&added.source_id)
            .await
            .unwrap()
            .as_deref(),
        Some("https://two.example.invalid/opds"),
        "并发的配置写入必须落地"
    );
}
