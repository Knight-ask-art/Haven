//! 用户登记目录的**真实搜索路径**证据（`cfg(test)`）。
//!
//! 这些用例不直接调用投影 helper，而是让 `OpdsSearchParticipant::search_for` 走完整链路：
//! 根目录 → OpenSearch 描述文件 → 搜索响应 → 解析出的 `pic` → 受控图片登记 → 卡片。
//! 它们证明的是**参与者这一段流水线**：只有三次目录请求、卡片元数据、登记端口收到的
//! 「来源身份 + 上游图片地址」、Wire 上只有 opaque 海报地址、取消之后不再登记。
//!
//! 这里**不**证明 URL 策略与 DNS 判定：夹具地址全是回环地址，全部经过
//! `allow_loopback_fixture` 这条显式 `cfg(test)` 缝隙；缝隙把回环地址直接钉到字面地址，
//! 因而既不执行 `UserProvidedSource` 出站策略、也不做 DNS 解析（见 `fixture` 模块）。
//! 「登记来源可访问用户明确配置的本机 endpoint、匿名访问仍受固定策略限制」与来源
//! origin 护栏由独立的生产路径用例钉住：
//! `request_tests::registered_loopback_catalog_uses_user_source_policy_without_the_test_seam`
//! 与 `tests::custom_opds_candidates_stay_on_the_registered_authority`。本文件的夹具缝隙
//! 只为完整搜索流水线构造离线目录，不代表生产策略的地址判定。
//!
//! 夹具里的 OpenSearch 模板与封面地址刻意写成**绝对 http** 与**协议相对**两种形态：
//! 旧实现会把它们改写成 `https://`，从而让搜索地址与卡片图片来源落到另一个 origin，
//! 于是这些断言会直接失败——这正是本次修正要钉住的缺陷。

use std::sync::{Arc, Mutex};

use haven_application::services::OPDS_CANDIDATE_PREFIX;
use haven_application::services::SourceRegistryService;
use haven_application::services::search_source::SearchSourceParticipant;

use super::fixture::{
    ArtworkOutcome, FixtureResponse, FixtureServer, RecordingImageProxy, counting_resolver,
    seeded_opds_registry,
};
use super::url::urlencode;
use super::{OpdsClient, OpdsSearchParticipant};

/// 根目录：feed 级 `rel="search"` 指向 OpenSearch 描述文件。
fn root_feed() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:haven:fixture:root</id>
  <title>自建书库</title>
  <link rel="search" type="application/atom+xml" href="/opensearch.xml"/>
</feed>"#
        .to_owned()
}

/// OpenSearch 描述文件：模板是**绝对 http** 地址（旧实现会把它升级成 https）。
fn opensearch_description(origin: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>自建书库</ShortName>
  <Url type="application/atom+xml" template="{origin}/search.opds?q={{searchTerms}}"/>
</OpenSearchDescription>"#
    )
}

/// 搜索结果：两条带封面的书目（绝对 http 与协议相对各一）+ 一条无封面的 EPUB 条目。
fn search_results(origin: &str) -> String {
    let authority = origin.trim_start_matches("http://");
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>{origin}/books/84.opds</id>
    <title>Frankenstein</title>
    <author><name>Mary Shelley</name></author>
    <link type="image/png" rel="http://opds-spec.org/image/thumbnail" href="{origin}/covers/84.png"/>
  </entry>
  <entry>
    <id>{origin}/books/85.opds</id>
    <title>Dracula</title>
    <link type="image/png" rel="http://opds-spec.org/image/thumbnail" href="//{authority}/covers/85.png"/>
  </entry>
  <entry>
    <id>{origin}/files/9.epub</id>
    <title>Direct Download</title>
    <link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="/files/9.epub"/>
  </entry>
</feed>"#
    )
}

fn catalog_server() -> FixtureServer {
    FixtureServer::start(|origin| {
        vec![
            ("/opds".to_owned(), FixtureResponse::atom(root_feed())),
            (
                "/opensearch.xml".to_owned(),
                FixtureResponse::opensearch(opensearch_description(origin)),
            ),
            (
                "/search.opds".to_owned(),
                FixtureResponse::atom(search_results(origin)),
            ),
        ]
    })
}

#[tokio::test]
async fn search_accepts_empty_feed_and_urn_book_with_external_cover() {
    for (xml, count) in [
        (r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#, 0),
        (
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>urn:uuid:book-1</id><title>URN Book</title><link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="/book.epub"/><link rel="http://opds-spec.org/image" type="image/png" href="https://cdn.example.invalid/cover.png"/></entry></feed>"#,
            1,
        ),
    ] {
        let server = FixtureServer::start(move |origin| {
            vec![
                ("/opds".to_owned(), FixtureResponse::atom(root_feed())),
                (
                    "/opensearch.xml".to_owned(),
                    FixtureResponse::opensearch(opensearch_description(origin)),
                ),
                (
                    "/search.opds".to_owned(),
                    FixtureResponse::atom(xml.to_owned()),
                ),
            ]
        });
        let (registry, source_id) =
            seeded_opds_registry(&format!("{}/opds", server.origin()), false).await;
        let client = Arc::new(
            OpdsClient::new()
                .unwrap()
                .with_loopback_fixture()
                .with_source_registry(registry.clone()),
        );
        let participant = OpdsSearchParticipant::new(source_id.clone(), registry, client);
        let cards = participant
            .search_for(&source_id, "book", 10, &|| false)
            .await
            .unwrap();
        assert_eq!(cards.len(), count);
        if count == 1 {
            assert_eq!(cards[0].title, "URN Book");
        }
    }
}

#[tokio::test]
async fn search_preserves_the_root_directory_error() {
    let server = FixtureServer::start(|_| Vec::new());
    let endpoint = format!("{}/missing", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new()
        .unwrap()
        .with_loopback_fixture()
        .with_source_registry(registry);
    let expected = client
        .search_url(Some(&source_id), &endpoint, "book")
        .await
        .unwrap_err();
    let actual = client
        .search_entries(Some(&source_id), &endpoint, "book", 10)
        .await
        .unwrap_err();
    assert_eq!(actual.code(), expected.code());
    assert_eq!(actual.user_message(), expected.user_message());
}

#[test]
fn search_feed_validation_rejects_broken_xml_and_html() {
    for xml in [
        "<html><body>login</body></html>",
        "<feed><entry></feed>",
        "<feed>",
        "<feed/><feed/>",
    ] {
        assert!(!super::valid_search_feed(xml), "{xml}");
    }
    assert!(super::valid_search_feed("<?xml version=\"1.0\"?><feed/>"));
}

#[test]
fn basic_auth_uses_saved_username_and_legacy_secret_requires_reconfiguration() {
    let client = reqwest::Client::new();
    let request = super::apply_opds_auth(
        client.get("https://catalog.example.invalid/opds"),
        "haven-opds-basic-v1\nreader\nsynthetic:password",
    )
    .unwrap()
    .build()
    .unwrap();
    use base64::Engine;
    let encoded = request.headers()[reqwest::header::AUTHORIZATION]
        .to_str()
        .unwrap()
        .strip_prefix("Basic ")
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
        b"reader:synthetic:password"
    );
    let result = super::apply_opds_auth(
        client.get("https://catalog.example.invalid/opds"),
        "old-password",
    );
    assert_eq!(
        result.err().unwrap().code().as_str(),
        "CREDENTIAL_RECONFIGURATION_REQUIRED"
    );
}

struct CatalogFixture {
    server: FixtureServer,
    client: Arc<OpdsClient>,
    registry: SourceRegistryService,
    registry_calls: Arc<Mutex<Vec<String>>>,
    source_id: String,
    origin: String,
}

async fn catalog_fixture() -> CatalogFixture {
    let server = catalog_server();
    let origin = server.origin().to_owned();
    let endpoint = format!("{origin}/opds");
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let registry_calls = Arc::new(Mutex::new(Vec::new()));
    let client = Arc::new(
        OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry.clone())
            .with_credential_resolver(counting_resolver(registry_calls.clone())),
    );
    CatalogFixture {
        server,
        client,
        registry,
        registry_calls,
        source_id,
        origin,
    }
}

impl CatalogFixture {
    fn participant(&self, proxy: Option<Arc<RecordingImageProxy>>) -> OpdsSearchParticipant {
        let participant = OpdsSearchParticipant::new(
            self.source_id.clone(),
            self.registry.clone(),
            self.client.clone(),
        );
        match proxy {
            Some(proxy) => participant.with_image_proxy(proxy),
            None => participant,
        }
    }
}

/// 完整链路：根目录 → OpenSearch 描述文件 → 搜索响应 → 卡片海报。
///
/// 只断言**受控**的图片事实：登记用的来源身份是分发到的 `sourceId`，卡片只拿
/// `haven://artwork/<opaque>`，上游图片地址不进 Wire；目录请求只有三次，**没有**任何
/// 逐条图片抓取；目录凭据解析器一次都没有被调用（图片路径不经过目录认证）。
#[tokio::test]
async fn opds_search_for_registers_catalog_artwork_through_the_real_search_path() {
    let fixture = catalog_fixture().await;
    let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
    let participant = fixture.participant(Some(proxy.clone()));

    let not_cancelled = || false;
    let cards = participant
        .search_for(&fixture.source_id, "frankenstein", 10, &not_cancelled)
        .await
        .expect("夹具目录必须可搜索");

    assert_eq!(cards.len(), 3, "两条有封面的书目 + 一条 EPUB 条目");
    assert_eq!(cards[0].title, "Frankenstein");
    assert_eq!(cards[1].title, "Dracula");
    assert_eq!(
        cards[0].poster_uri.as_deref(),
        Some("haven://artwork/artwork-1")
    );
    assert_eq!(
        cards[1].poster_uri.as_deref(),
        Some("haven://artwork/artwork-1")
    );
    assert_eq!(cards[2].poster_uri, None, "没有图片字段的条目保持无海报");
    for card in &cards {
        assert!(
            card.work_id.starts_with(OPDS_CANDIDATE_PREFIX),
            "候选句柄必须带既有的候选前缀: {}",
            card.work_id
        );
    }
    assert_eq!(
        cards[0].work_id,
        format!(
            "{OPDS_CANDIDATE_PREFIX}{}\u{1}{}",
            fixture.source_id,
            urlencode(&format!("{}/books/84.opds", fixture.origin))
        )
    );

    // 登记使用**分发到的来源身份**与上游解析出来的原始 http 地址（协议相对的封面也必须
    // 继承目录的 scheme，而不是被改写成 https）。
    assert_eq!(
        proxy.calls(),
        vec![
            (
                fixture.source_id.clone(),
                format!("{}/covers/84.png", fixture.origin)
            ),
            (
                fixture.source_id.clone(),
                format!("{}/covers/85.png", fixture.origin)
            ),
        ]
    );

    // 只有三次目录请求：根目录、描述文件、搜索；没有任何一条封面地址被请求过
    // （搜索阶段不逐条抓图）。
    assert_eq!(
        fixture.server.paths(),
        vec!["/opds", "/opensearch.xml", "/search.opds"]
    );
    assert_eq!(
        fixture.server.requests()[2].target(),
        "/search.opds?q=frankenstein"
    );
    assert!(
        fixture
            .server
            .requests()
            .iter()
            .all(|request| !request.path().starts_with("/covers/")),
        "搜索阶段不得为任何结果发起图片请求"
    );

    // Wire 上只有 opaque 地址。
    let wire = serde_json::to_string(&cards).unwrap();
    assert!(wire.contains("haven://artwork/artwork-1"));
    assert!(!wire.contains("/covers/84.png"));
    assert!(!wire.contains("/covers/85.png"));

    // 图片登记不经过目录认证：登记端口**只**接收「来源身份 + 上游图片地址」两个参数
    // （`ImageProxyRepository::register(source_id, target_url)`），secret 没有第三个入口。
    // 本夹具登记的是**未配置凭据**的来源，所以这条断言证明的是「匿名目录的图片路径不解析
    // 凭据」；它不是「配了凭据的来源也不会把 secret 交给图片路径」的验收——后者是登记端口
    // 的形状事实，由上面那次 `proxy.calls()` 断言（参数只有来源身份与地址）与 `opds::tests`
    // 里的 Artwork 投影用例共同钉住。本用例不做凭据来源的断言。
    assert!(
        fixture.registry_calls.lock().unwrap().is_empty(),
        "匿名目录的图片路径不得解析目录凭据"
    );
}

/// 缺失 / 被拒 / 登记失败 / id 形状非法：卡片保持元数据、只是没有海报。
#[tokio::test]
async fn opds_search_for_falls_back_to_no_artwork_for_every_registration_outcome() {
    let fixture = catalog_fixture().await;
    let not_cancelled = || false;
    for (outcome, expected_poster) in [
        (
            ArtworkOutcome::Registered,
            Some("haven://artwork/artwork-1"),
        ),
        (ArtworkOutcome::RegisteredInvalidId, None),
        (ArtworkOutcome::Rejected, None),
        (ArtworkOutcome::Failed, None),
    ] {
        let proxy = RecordingImageProxy::new(outcome);
        let participant = fixture.participant(Some(proxy.clone()));
        let cards = participant
            .search_for(&fixture.source_id, "frankenstein", 10, &not_cancelled)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "catalog fixture error: {error:?}; connections={}; requests={}",
                    fixture.server.connections(),
                    fixture.server.requests().len()
                )
            });
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].poster_uri.as_deref(), expected_poster);
        assert_eq!(cards[0].title, "Frankenstein", "元数据必须原样保留");
        assert_eq!(
            proxy.calls().len(),
            2,
            "两条带封面的条目都会尝试登记，结果由登记端口决定"
        );
    }

    // 没有注入登记端口的参与者照常工作，只是没有海报。
    let participant = fixture.participant(None);
    let cards = participant
        .search_for(&fixture.source_id, "frankenstein", 10, &not_cancelled)
        .await
        .unwrap();
    assert_eq!(cards.len(), 3);
    assert!(cards.iter().all(|card| card.poster_uri.is_none()));
}

/// 取消之后不再新增登记：已经登记过的 opaque 身份不被删除，但后续条目不再产生映射。
#[tokio::test]
async fn opds_search_for_stops_registering_after_the_first_cancelled_poll() {
    let fixture = catalog_fixture().await;
    let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
    // 取消检查器在第一次成功登记之后翻转：这样「取消发生在登记之间」是确定性的，
    // 不依赖调度顺序或计时。
    let gate = proxy.clone();
    let is_cancelled = move || !gate.calls().is_empty();
    let participant = fixture.participant(Some(proxy.clone()));

    let cards = participant
        .search_for(&fixture.source_id, "frankenstein", 10, &is_cancelled)
        .await
        .unwrap();
    assert_eq!(cards.len(), 3);
    assert_eq!(
        cards[0].poster_uri.as_deref(),
        Some("haven://artwork/artwork-1")
    );
    assert_eq!(cards[1].poster_uri, None);
    assert_eq!(cards[1].title, "Dracula", "取消不影响已返回的元数据");
    assert_eq!(
        proxy.calls(),
        vec![(
            fixture.source_id.clone(),
            format!("{}/covers/84.png", fixture.origin)
        )],
        "取消之后不得继续登记图片"
    );
}
