//! 观测 → 投影/落库的持久化事实。
//!
//! 覆盖：真实观测的指标投影、有界窗口、损坏行的诚实降级、重启（新实例）恢复、
//! 只读装配面既不探测也不伪造，以及健康写入永不改写来源配置。

use std::sync::Arc;

use haven_application::services::source_registry::{SourceHealthRecorder, SourceRegistryService};
use haven_application::wire::{SourceHealthDto, SourceRegistrySetRequest};
use haven_common::UtcMillis;
use haven_domain::contracts::{SettingsRepository, SettingsRow};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};

use super::fixtures::{
    HEALTH_WINDOW, LATENCY_MS, current_revision, descriptor_of, health_section, obs,
    registry_with_health, stored_health_json, stored_health_row,
};

#[tokio::test]
async fn real_observations_project_into_health_metrics() {
    let (registry, _repos) = registry_with_health();
    let revision = current_revision(&registry, "mangadex").await;

    registry
        .record_search_observation(obs("mangadex", revision.clone(), 1_000, true))
        .await
        .unwrap();
    let projected = descriptor_of(&registry, "mangadex").await;
    assert_eq!(projected.health, SourceHealthDto::Ok);
    assert_eq!(projected.success_rate, Some(1.0));
    assert_eq!(
        projected.last_checked.as_deref(),
        Some("1970-01-01T00:00:01Z")
    );
    assert_eq!(projected.latency_ms, Some(LATENCY_MS));
    assert!(projected.enabled, "健康事实永远不改写启用状态");

    // 连续失败：1 次、2 次是降级，第 3 次起判为不可用；成功率按整个窗口算。
    for (started_at, expected) in [
        (2_000, SourceHealthDto::Degraded),
        (3_000, SourceHealthDto::Degraded),
        (4_000, SourceHealthDto::Down),
    ] {
        registry
            .record_search_observation(obs("mangadex", revision.clone(), started_at, false))
            .await
            .unwrap();
        assert_eq!(descriptor_of(&registry, "mangadex").await.health, expected);
    }
    let projected = descriptor_of(&registry, "mangadex").await;
    assert_eq!(projected.success_rate, Some(0.25));
    assert!(projected.enabled, "来源连续失败也不得关掉用户的选择");
}

#[tokio::test]
async fn one_search_batch_persists_observations_for_each_source() {
    let (registry, _repos) = registry_with_health();
    let cms_revision = current_revision(&registry, "cms10").await;
    let manga_revision = current_revision(&registry, "mangadex").await;

    registry
        .record_search_observations(vec![
            obs("cms10", cms_revision, 1_000, true),
            obs("mangadex", manga_revision, 2_000, false),
        ])
        .await
        .unwrap();

    assert_eq!(
        descriptor_of(&registry, "cms10").await.health,
        SourceHealthDto::Ok
    );
    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Degraded
    );
}

/// 健康写入只碰 `source_health:<sourceId>` 一行：已启用状态与用户配置的端点都不动。
#[tokio::test]
async fn health_writes_never_toggle_enabled_or_endpoints() {
    let (registry, _repos) = registry_with_health();
    registry
        .set_endpoint("cms10", "https://a.example.invalid/api.php")
        .await
        .unwrap();
    registry
        .set(SourceRegistrySetRequest {
            source_id: "cms10".into(),
            enabled: false,
        })
        .await
        .unwrap();
    let revision = current_revision(&registry, "cms10").await;

    registry
        .record_search_observation(obs("cms10", revision, 1_000, false))
        .await
        .unwrap();

    let projected = descriptor_of(&registry, "cms10").await;
    assert!(!projected.enabled, "停用状态不得被迟到的结果改写");
    assert!(projected.endpoint_configured);
    assert_eq!(projected.health, SourceHealthDto::Degraded);
    assert_eq!(
        registry.endpoint("cms10").await.unwrap().as_deref(),
        Some("https://a.example.invalid/api.php")
    );
}

/// `list` 只读取已落库事实：既不做探测，也不顺手播种健康行。
#[tokio::test]
async fn listing_sources_never_probes_or_seeds_health() {
    let (registry, repos) = registry_with_health();
    let listed = registry.list().await.unwrap();
    assert!(!listed.sources.is_empty());
    for source in &listed.sources {
        assert_eq!(source.health, SourceHealthDto::Unknown);
        assert_eq!(source.last_checked, None);
        assert_eq!(source.latency_ms, None);
        assert_eq!(source.success_rate, None);
        assert!(
            stored_health_row(&repos, &source.source_id).await.is_none(),
            "list 只读取已落库事实：不得探测，也不得写入健康行"
        );
    }
}

/// 诊断数据损坏时诚实返回 unknown（不报错、不猜），下一次真实观测覆盖它。
#[tokio::test]
async fn corrupt_health_rows_project_unknown_and_are_replaced() {
    let (registry, repos) = registry_with_health();
    SettingsRepository::upsert(
        repos.as_ref(),
        &SettingsRow {
            section: health_section("mangadex"),
            schema_version: 1,
            revision: "corrupt-health-1".to_owned(),
            data_json: "{ not json".to_owned(),
            updated_at: UtcMillis(1),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Unknown
    );
    registry
        .record_search_observation(obs("mangadex", None, 9_000, true))
        .await
        .unwrap();
    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Ok
    );
}

/// 重启等价：新服务实例读取同一份持久化健康事实。
///
/// 这里刻意走历史别名 `with_health_uow`（健康切片先落地时的方法名）：它与
/// `with_settings_uow` 必须始终等价，注入哪一个都要能读到同一份事实。
#[tokio::test]
async fn health_records_survive_a_new_service_instance() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let first = SourceRegistryService::new(repos.clone())
        .with_health_uow(Arc::new(SqliteSettingsUoW::new(db.clone())));
    let revision = current_revision(&first, "mangadex").await;
    first
        .record_search_observation(obs("mangadex", revision, 1_000, true))
        .await
        .unwrap();

    let second = SourceRegistryService::new(repos)
        .with_health_uow(Arc::new(SqliteSettingsUoW::new(db.clone())));
    let projected = descriptor_of(&second, "mangadex").await;
    assert_eq!(projected.health, SourceHealthDto::Ok);
    assert_eq!(projected.success_rate, Some(1.0));
}

/// 未注入 `SettingsUoW` 的组装面仍然可用，只是健康诚实保持 unknown。
#[tokio::test]
async fn registry_without_a_write_channel_never_fabricates_health() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let registry = SourceRegistryService::new(Arc::new(SqliteRepositories::new(db)));
    registry
        .record_search_observation(obs("mangadex", None, 1_000, true))
        .await
        .unwrap();
    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Unknown
    );
}

#[tokio::test]
async fn health_window_is_bounded_and_drops_the_oldest_observations() {
    let (registry, repos) = registry_with_health();
    let revision = current_revision(&registry, "mangadex").await;

    // 先 5 次成功，再 20 次失败：窗口只有 20 个位置，成功率必须只看最近这 20 次。
    for index in 0..5_i64 {
        registry
            .record_search_observation(obs("mangadex", revision.clone(), 1_000 + index, true))
            .await
            .unwrap();
    }
    for index in 0..HEALTH_WINDOW as i64 {
        registry
            .record_search_observation(obs("mangadex", revision.clone(), 2_000 + index, false))
            .await
            .unwrap();
    }

    let window = stored_health_json(&repos, "mangadex").await["window"]
        .as_array()
        .expect("窗口必须是数组")
        .clone();
    assert_eq!(window.len(), HEALTH_WINDOW, "窗口必须有界");
    assert!(
        window.iter().all(|succeeded| !succeeded.as_bool().unwrap()),
        "最早的 5 次成功必须已经被挤出窗口"
    );

    let projected = descriptor_of(&registry, "mangadex").await;
    assert_eq!(projected.success_rate, Some(0.0));
    assert_eq!(projected.health, SourceHealthDto::Down);
    assert_eq!(projected.latency_ms, Some(LATENCY_MS));
}
