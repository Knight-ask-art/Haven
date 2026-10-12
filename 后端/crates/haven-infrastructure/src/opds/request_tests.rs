//! 一次 OPDS 请求的授权判定证据（`cfg(test)`）。
//!
//! 四件事各有独立用例，互不掩盖：
//! 1. 匿名**公开** HTTP 目录的地址构造保留自己的 scheme 与端口（不需要任何回环缝隙）；
//! 2. 已登记的自定义目录在生产路径可访问用户明确配置的本机端点，而匿名访问仍拒绝；
//! 3. 异步凭据解析之后的权威上下文复核对 `Some` 与 `None` 两种结果都成立，且失效的
//!    上下文不会建立任何 socket；
//! 4. 解析期间来源被**删除**（端点/版本变化之外的另一类变化）同样让旧上下文作废，
//!    失败同样发生在任何 socket 之前。

use std::sync::Arc;

use haven_application::wire::SourceUpdateRequest;

use super::fixture::{
    FixtureResponse, FixtureServer, opds_sources_payload, paused_resolver, provider_endpoint,
    seeded_opds_registry, seeded_opds_registry_with_settings,
    sources_payload_without_custom_sources,
};
use super::request::OpdsRequestContext;
use super::url::absolutize;
use super::{OpdsClient, validate_opds_url};

/// 匿名公开 HTTP 目录：地址构造保留 scheme 与端口，策略也不要求它升级成 HTTPS。
#[test]
fn anonymous_public_http_catalog_keeps_its_own_scheme_and_port() {
    let anonymous = OpdsRequestContext::anonymous();
    // 匿名请求走固定严格策略：公开主机的 http、常见端口都照常接受（不需要凭据路径的
    // HTTPS 强制），地址原样保留。
    let endpoint = "http://catalog.example.invalid:8080/opds";
    let validated = validate_opds_url(endpoint, &anonymous).expect("匿名公开 HTTP 目录是合法地址");
    assert_eq!(validated.as_str(), endpoint);
    // 用户登记的自托管目录可以用任意非零端口：链接构造必须原样保留那个端口，而不是
    // 改写成别的 scheme/端口（否则来源护栏会把同源链接判成越界）。
    let self_hosted = "http://catalog.example.invalid:8096/opds";
    assert_eq!(
        absolutize(self_hosted, "/books/9.opds").as_deref(),
        Some("http://catalog.example.invalid:8096/books/9.opds")
    );
    // 公开 HTTPS 目录同样保持原样（不会被改写端口或 scheme）。
    let secure = "https://catalog.example.invalid:8443/opds";
    assert_eq!(
        validate_opds_url(secure, &anonymous).unwrap().as_str(),
        secure
    );
}

/// 已登记的自定义目录可以在生产路径访问用户明确配置的本机端点；匿名访问仍走固定来源
/// 策略并拒绝回环地址。这个用例不启用 `cfg(test)` 回环缝隙，直接验证实际来源策略和请求。
#[tokio::test]
async fn registered_loopback_catalog_uses_user_source_policy_without_the_test_seam() {
    let server = FixtureServer::start(|_| {
        vec![(
            "/opds".to_owned(),
            FixtureResponse::atom(
                r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Local</title></feed>"#,
            ),
        )]
    });
    let endpoint = format!("{}/opds", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;

    let production = OpdsClient::new()
        .unwrap()
        .with_source_registry(registry.clone());
    let context = production
        .request_context(Some(&source_id))
        .await
        .expect("已登记的自定义 OPDS 书源必须得到上下文");
    assert!(
        production.validate_request_url(&endpoint, &context).is_ok(),
        "登记来源的本机 endpoint 应使用用户来源策略"
    );
    production
        .get_feed(&context, &endpoint)
        .await
        .expect("生产请求路径应能访问用户明确配置的本机目录");
    assert_eq!(server.paths(), vec!["/opds"]);

    assert!(
        production
            .validate_request_url(&endpoint, &OpdsRequestContext::anonymous())
            .is_err(),
        "匿名路径同样不得接受回环目录地址"
    );
}

/// 解析结果为 `None` 时也必须复核：解析期间端点/版本变化 → 本次请求作废，且旧上下文
/// 连一个 socket 都不会建立。
#[tokio::test]
async fn credential_resolution_returning_none_still_rechecks_the_request_context() {
    let server = FixtureServer::start(|_| {
        vec![(
            "/feed".to_owned(),
            FixtureResponse::atom(r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#),
        )]
    });
    let secure_origin = server.origin().replacen("http://", "https://", 1);
    let (registry, source_id) =
        seeded_opds_registry(&provider_endpoint(&secure_origin), true).await;
    let (resolver, entered, release) = paused_resolver(None);
    let client = Arc::new(
        OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry.clone())
            .with_credential_resolver(resolver),
    );
    let context = client
        .request_context(Some(&source_id))
        .await
        .expect("已登记的自定义 OPDS 书源必须得到上下文");
    let url = format!("{secure_origin}/feed");

    let task = tokio::spawn({
        let client = client.clone();
        let context = context.clone();
        let url = url.clone();
        async move { client.get_feed(&context, &url).await }
    });
    entered.await.expect("解析器必须被调用并暂停");

    registry
        .update_custom_source(SourceUpdateRequest {
            source_id: source_id.clone(),
            display_name: None,
            endpoint: Some("https://other.example.invalid/opds".to_owned()),
        })
        .await
        .unwrap();
    let _ = release.send(());

    let error = task
        .await
        .unwrap()
        .expect_err("解析结果为空也必须复核权威上下文");
    assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    assert!(
        error.user_message().contains("变更"),
        "必须是可识别的上下文失效错误，实际: {}",
        error.user_message()
    );
    assert_eq!(server.connections(), 0, "失效上下文不得建立任何 socket");
    assert!(server.requests().is_empty());
}

/// 解析结果非空、**端点一字未改**、只有配置版本变化：旧上下文一样作废，旧 secret 不会
/// 被发往任何地址。
#[tokio::test]
async fn credential_resolution_rechecks_a_revision_only_change() {
    let server = FixtureServer::start(|_| {
        vec![(
            "/feed".to_owned(),
            FixtureResponse::atom(r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#),
        )]
    });
    let secure_origin = server.origin().replacen("http://", "https://", 1);
    let endpoint = provider_endpoint(&secure_origin);
    let (registry, source_id, settings) = seeded_opds_registry_with_settings(&endpoint, true).await;
    let (resolver, entered, release) = paused_resolver(Some("p@ss:word"));
    let client = Arc::new(
        OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry.clone())
            .with_credential_resolver(resolver),
    );
    let context = client
        .request_context(Some(&source_id))
        .await
        .expect("已登记的自定义 OPDS 书源必须得到上下文");
    let url = format!("{secure_origin}/feed");

    let task = tokio::spawn({
        let client = client.clone();
        let context = context.clone();
        let url = url.clone();
        async move { client.get_feed(&context, &url).await }
    });
    entered.await.expect("解析器必须被调用并暂停");

    // 端点与凭据引用都不动，只换配置版本：真实写入路径里端点/凭据变更正是这样作废旧
    // 观测的（版本比较的是身份，不是「有没有变回过」）。
    for revision in ["rev-2", "rev-3"] {
        settings
            .write(&opds_sources_payload(&endpoint, &source_id, true, revision))
            .await;
    }
    let _ = release.send(());

    let error = task
        .await
        .unwrap()
        .expect_err("配置版本变化也必须作废旧上下文");
    assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    assert!(
        error.user_message().contains("变更"),
        "必须是可识别的上下文失效错误，实际: {}",
        error.user_message()
    );
    assert_eq!(server.connections(), 0, "失效上下文不得建立任何 socket");
}

/// 解析期间来源被**删除**（不是换端点、也不是换版本）：注册表里不再有这条记录，旧上下文
/// 一样作废，失败同样发生在任何 socket 之前。
///
/// 夹具服务器本身是完好的（同一个 `/feed` 路由在其它用例里正常应答），所以这里失败的必然
/// 是权威复核，而不是传输或夹具。用例只在凭据解析器主动暂停的窗口里改写设置行，因此不需要
/// 任何计时等待或超时升级。
#[tokio::test]
async fn credential_resolution_rechecks_a_removed_source() {
    let server = FixtureServer::start(|_| {
        vec![(
            "/feed".to_owned(),
            FixtureResponse::atom(r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#),
        )]
    });
    let secure_origin = server.origin().replacen("http://", "https://", 1);
    let endpoint = provider_endpoint(&secure_origin);
    let (registry, source_id, settings) = seeded_opds_registry_with_settings(&endpoint, true).await;
    let (resolver, entered, release) = paused_resolver(Some("p@ss:word"));
    let client = Arc::new(
        OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry.clone())
            .with_credential_resolver(resolver),
    );
    let context = client
        .request_context(Some(&source_id))
        .await
        .expect("已登记的自定义 OPDS 书源必须得到上下文");
    let url = format!("{secure_origin}/feed");

    let task = tokio::spawn({
        let client = client.clone();
        let context = context.clone();
        let url = url.clone();
        async move { client.get_feed(&context, &url).await }
    });
    entered.await.expect("解析器必须被调用并暂停");

    // 暂停窗口里把来源整行删除：注册表不再承认这个 sourceId。
    settings
        .write(&sources_payload_without_custom_sources())
        .await;
    assert!(
        registry
            .registered_source_request_snapshot(&source_id)
            .await
            .unwrap()
            .is_none(),
        "夹具必须真的把来源删掉（而不是只改了它的字段）"
    );
    let _ = release.send(());

    let error = task
        .await
        .unwrap()
        .expect_err("来源被删除后旧上下文必须作废");
    // 删除后注册表不再承认这个身份：复核以「未知来源目录」fail closed，且不可重试。
    // 端点/版本变化仍沿用可重试的 SOURCE_UNAVAILABLE；这里不得发出旧来源请求。
    assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    assert!(
        error.user_message().contains("未知来源"),
        "必须是可识别的来源失效错误，实际: {}",
        error.user_message()
    );
    assert!(!error.retryable(), "来源失效是稳定判定，不得标记为可重试");
    assert_eq!(server.connections(), 0, "失效上下文不得建立任何 socket");
    assert!(server.requests().is_empty());
}
