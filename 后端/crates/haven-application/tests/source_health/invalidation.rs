//! 配置变更使旧观测作废（提示新鲜度栅栏）。
//!
//! 覆盖：乱序完成、端点 A→B→A、同值重写幂等、编辑一个来源不影响别人、凭据变更、
//! 来源删除后的迟到观测，以及「版本变化后从空窗口重新开始」。

use haven_application::services::source_config_cache::InMemorySourceConfigCache;
use haven_application::services::source_registry::SourceHealthRecorder;
use haven_application::wire::{SourceHealthDto, SourceSetCredentialRequest};

use super::fixtures::{
    NoopCredentialStore, current_revision, descriptor_of, obs, registry_with_health,
    stored_health_json, stored_health_row,
};

#[tokio::test]
async fn out_of_order_completion_does_not_regress_a_newer_check() {
    let (registry, _repos) = registry_with_health();
    let revision = current_revision(&registry, "mangadex").await;

    // 后开始的检查先完成。
    registry
        .record_search_observation(obs("mangadex", revision.clone(), 5_000, true))
        .await
        .unwrap();
    // 先开始的检查晚到：更早的时刻，不得回退已落库的更新观测。
    registry
        .record_search_observation(obs("mangadex", revision, 4_000, false))
        .await
        .unwrap();

    let projected = descriptor_of(&registry, "mangadex").await;
    assert_eq!(projected.health, SourceHealthDto::Ok);
    assert_eq!(projected.success_rate, Some(1.0));
    assert_eq!(
        projected.last_checked.as_deref(),
        Some("1970-01-01T00:00:05Z")
    );
}

/// 请求飞行中用户把地址改到 B 又改回 A：版本仍然推进，携带旧版本的迟到观测不得
/// 把（已经不是它探测过的）地址标成健康。
#[tokio::test]
async fn a_to_b_to_a_endpoint_changes_invalidate_stale_outcomes() {
    let (registry, _repos) = registry_with_health();
    assert!(
        registry
            .set_endpoint("cms10", "https://a.example.invalid/api.php")
            .await
            .unwrap()
    );
    let revision_a = current_revision(&registry, "cms10").await;
    assert!(revision_a.is_some(), "写入端点必须产生配置版本");

    registry
        .set_endpoint("cms10", "https://b.example.invalid/api.php")
        .await
        .unwrap();
    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();
    let revision_a_again = current_revision(&registry, "cms10").await;
    assert_ne!(revision_a, revision_a_again, "A→B→A 必须产生新版本");

    registry
        .record_search_observation(obs("cms10", revision_a, 1_000, true))
        .await
        .unwrap();
    let projected = descriptor_of(&registry, "cms10").await;
    assert_eq!(projected.health, SourceHealthDto::Unknown);
    assert_eq!(projected.success_rate, None);
    assert_eq!(projected.last_checked, None);

    // 当前版本的观测照常落地。
    registry
        .record_search_observation(obs("cms10", revision_a_again, 2_000, true))
        .await
        .unwrap();
    assert_eq!(
        descriptor_of(&registry, "cms10").await.health,
        SourceHealthDto::Ok
    );
}

/// 同值重复设置不产生新版本：健康历史不被无谓作废。
#[tokio::test]
async fn same_endpoint_rewrite_is_idempotent_and_keeps_health_history() {
    let (registry, _repos) = registry_with_health();
    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();
    let revision = current_revision(&registry, "cms10").await;
    registry
        .record_search_observation(obs("cms10", revision.clone(), 1_000, true))
        .await
        .unwrap();

    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();
    assert_eq!(current_revision(&registry, "cms10").await, revision);
    assert_eq!(
        descriptor_of(&registry, "cms10").await.health,
        SourceHealthDto::Ok
    );
}

#[tokio::test]
async fn editing_one_source_keeps_other_sources_history() {
    let (registry, _repos) = registry_with_health();
    let mangadex_revision = current_revision(&registry, "mangadex").await;
    registry
        .record_search_observation(obs("mangadex", mangadex_revision.clone(), 1_000, true))
        .await
        .unwrap();

    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();

    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Ok
    );
    assert_eq!(
        current_revision(&registry, "mangadex").await,
        mangadex_revision,
        "编辑一个来源不得推进其它来源的配置版本"
    );
}

/// 凭据变更同样推进版本（请求是否会带认证变了）：分发发生在凭据写入之前的观测作废，
/// 但「是否已配置凭据」这个事实不得被健康写入改写。
#[tokio::test]
async fn credential_change_invalidates_stale_outcomes_without_touching_the_option() {
    let (registry, _repos) = registry_with_health();
    let added = registry
        .add_custom_source("我的书源", "https://one.example.invalid/opds")
        .await
        .unwrap();
    let before = current_revision(&registry, &added.source_id).await;

    registry
        .set_custom_source_credential(
            &SourceSetCredentialRequest {
                username: None,
                source_id: added.source_id.clone(),
                secret: Some("pw".into()),
            },
            &NoopCredentialStore,
        )
        .await
        .unwrap();
    let after = current_revision(&registry, &added.source_id).await;
    assert_ne!(before, after, "凭据变更必须产生新版本");

    registry
        .record_search_observation(obs(&added.source_id, before, 1_000, true))
        .await
        .unwrap();
    let projected = descriptor_of(&registry, &added.source_id).await;
    assert_eq!(projected.health, SourceHealthDto::Unknown);
    assert!(projected.credential_configured, "健康写入不得改写凭据引用");

    registry
        .record_search_observation(obs(&added.source_id, after, 2_000, true))
        .await
        .unwrap();
    assert_eq!(
        descriptor_of(&registry, &added.source_id).await.health,
        SourceHealthDto::Ok
    );
}

#[tokio::test]
async fn observation_for_a_deleted_source_is_never_persisted() {
    let (registry, repos) = registry_with_health();
    let added = registry
        .add_custom_source("书源", "https://one.example.invalid/opds")
        .await
        .unwrap();
    let revision = current_revision(&registry, &added.source_id).await;

    registry
        .remove_custom_source(
            &added.source_id,
            &NoopCredentialStore,
            &InMemorySourceConfigCache::new(),
        )
        .await
        .unwrap();

    // 迟到的观测既不能写回健康，也不能复活已删除的来源。
    registry
        .record_search_observation(obs(&added.source_id, revision, 1_000, true))
        .await
        .unwrap();
    assert!(
        stored_health_row(&repos, &added.source_id).await.is_none(),
        "已删除的来源不得留下新的健康行"
    );
    assert!(
        !registry
            .list()
            .await
            .unwrap()
            .sources
            .iter()
            .any(|source| source.source_id == added.source_id)
    );
}

/// 版本变化后从**空窗口**重新开始：旧版本可能领先的时间戳不得压掉新版本的观测。
#[tokio::test]
async fn revision_change_starts_a_fresh_window_and_ignores_the_old_watermark() {
    let (registry, repos) = registry_with_health();
    assert!(
        registry
            .set_endpoint("cms10", "https://a.example.invalid/api.php")
            .await
            .unwrap()
    );
    let revision_a = current_revision(&registry, "cms10").await;

    // 旧版本连续失败到 Down，且它的检查时刻「领先」（乱序/时钟偏差都可能造成）。
    for started in [9_000, 9_001, 9_002] {
        registry
            .record_search_observation(obs("cms10", revision_a.clone(), started, false))
            .await
            .unwrap();
    }
    assert_eq!(
        descriptor_of(&registry, "cms10").await.health,
        SourceHealthDto::Down
    );

    registry
        .set_endpoint("cms10", "https://b.example.invalid/api.php")
        .await
        .unwrap();
    let revision_b = current_revision(&registry, "cms10").await;
    assert_ne!(revision_a, revision_b, "改地址必须产生新版本");

    registry
        .record_search_observation(obs("cms10", revision_b, 1_000, true))
        .await
        .unwrap();
    let projected = descriptor_of(&registry, "cms10").await;
    assert_eq!(
        projected.health,
        SourceHealthDto::Ok,
        "旧版本的未来时间戳不得压掉新版本的观测"
    );
    assert_eq!(
        projected.success_rate,
        Some(1.0),
        "新窗口里只有新版本的观测"
    );
    assert_eq!(
        projected.last_checked.as_deref(),
        Some("1970-01-01T00:00:01Z")
    );

    // 旧版本的迟到观测既不进窗口，也不回退新版本已经落库的时刻。
    registry
        .record_search_observation(obs("cms10", revision_a, 2_000, false))
        .await
        .unwrap();
    let projected = descriptor_of(&registry, "cms10").await;
    assert_eq!(projected.health, SourceHealthDto::Ok);
    assert_eq!(projected.success_rate, Some(1.0));
    assert_eq!(
        stored_health_json(&repos, "cms10").await["window"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "新窗口不得混入旧版本的观测"
    );
}
