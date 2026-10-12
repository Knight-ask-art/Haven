//! `sources` 段的首次写入初始化（真实 SQLite + 真实设置事务）。
//!
//! 稳定契约：section 的初始状态不取决于**哪一次写入先到**。第一次 `list` 之前的显式
//! 停用、第一次端点写入、第一次自定义来源登记，三者都必须先播种同一份出厂默认（内置
//! 端点 + 出厂启用集合），再应用这一次的变更；设置行一旦存在，出厂默认永不再播种。

use haven_application::services::source_registry::SourceRegistryService;
use haven_application::wire::SourceRegistrySetRequest;

use super::fixtures::{Harness, add_source, descriptor_of};

/// 出厂默认启用的固定来源（CMS10 需要用户先配置端点，因此不在其列）。
const FACTORY_ENABLED: [&str; 5] = [
    "opds_gutenberg",
    "mangadex",
    "arxiv",
    "europepmc",
    "wikisource",
];

async fn assert_factory_defaults_present(registry: &SourceRegistryService, except: &[&str]) {
    for source_id in FACTORY_ENABLED {
        if except.contains(&source_id) {
            continue;
        }
        assert!(
            descriptor_of(registry, source_id).await.enabled,
            "{source_id} 的出厂启用不取决于哪一次写入先到"
        );
    }
}

#[tokio::test]
async fn an_explicit_disable_before_the_first_list_survives_a_restart() {
    let harness = Harness::new();
    // 全新安装：设置行尚不存在时用户就停用了出厂启用的 Gutenberg。
    harness
        .registry
        .set(SourceRegistrySetRequest {
            source_id: "opds_gutenberg".to_owned(),
            enabled: false,
        })
        .await
        .unwrap();

    // 这次写入必须已经把出厂默认与用户选择一起落库。
    let stored = harness.stored_sources().await;
    assert!(
        stored["endpoints"]["opds_gutenberg"].is_string(),
        "出厂端点必须与出厂启用一起播种"
    );
    assert!(
        stored["enabledSources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|id| id != "opds_gutenberg"),
        "用户刚写入的停用选择必须已经落库"
    );

    // 重启等价：新实例读同一份持久化事实，list 不得把它重新打开。
    let restarted = harness.restart();
    let listed = restarted.list().await.unwrap();
    let gutenberg = listed
        .sources
        .iter()
        .find(|source| source.source_id == "opds_gutenberg")
        .unwrap();
    assert!(!gutenberg.enabled);
    assert!(gutenberg.endpoint_configured);
    assert_factory_defaults_present(&restarted, &["opds_gutenberg"]).await;
    assert!(
        !descriptor_of(&restarted, "cms10").await.enabled,
        "其余来源不受这次写入影响"
    );
}

#[tokio::test]
async fn stored_enable_choices_are_never_reseeded() {
    let harness = Harness::new();
    let _ = harness.registry.list().await.unwrap();
    harness
        .registry
        .set(SourceRegistrySetRequest {
            source_id: "mangadex".to_owned(),
            enabled: false,
        })
        .await
        .unwrap();

    let restarted = harness.restart();
    let listed = restarted.list().await.unwrap();
    for (source_id, expected) in [
        ("mangadex", false),
        ("opds_gutenberg", true),
        ("arxiv", true),
    ] {
        assert_eq!(
            listed
                .sources
                .iter()
                .find(|source| source.source_id == source_id)
                .unwrap()
                .enabled,
            expected,
            "{source_id} 的持久化选择不得被重新播种"
        );
    }
    // 反复读取也不会改变任何选择。
    let _ = harness.registry.list().await.unwrap();
    assert!(!descriptor_of(&harness.registry, "mangadex").await.enabled);
}

#[tokio::test]
async fn the_first_endpoint_write_seeds_the_same_factory_defaults() {
    let harness = Harness::new();
    harness
        .registry
        .set_endpoint("cms10", "https://example.invalid/api.php/provide/vod")
        .await
        .unwrap();

    let restarted = harness.restart();
    let cms10 = descriptor_of(&restarted, "cms10").await;
    assert!(cms10.endpoint_configured);
    assert!(!cms10.enabled, "仅写入端点不得自动启用 CMS10");
    assert_factory_defaults_present(&restarted, &[]).await;
}

#[tokio::test]
async fn the_first_custom_source_registration_seeds_the_same_factory_defaults() {
    let harness = Harness::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let restarted = harness.restart();
    assert!(!descriptor_of(&restarted, &source_id).await.enabled);
    assert_factory_defaults_present(&restarted, &[]).await;
}
