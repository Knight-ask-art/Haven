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

#[tokio::test]
async fn registered_directory_slashes_survive_add_update_and_real_search() {
    let server = FixtureServer::start(|_| {
        ["/opds", "/library"].into_iter().flat_map(|base| [
            (format!("{base}/"), FixtureResponse::atom(r#"<feed><link rel="search" href="search.xml"/></feed>"#)),
            (format!("{base}/search.xml"), FixtureResponse::opensearch(r#"<OpenSearchDescription><Url type="application/atom+xml" template="results?q={searchTerms}"/></OpenSearchDescription>"#)),
            (format!("{base}/results"), FixtureResponse::atom(r#"<feed><entry><id>urn:book</id><title>Book</title></entry></feed>"#)),
        ]).collect()
    });
    let repos = Arc::new(crate::db::repos::SqliteRepositories::new(Arc::new(
        crate::Db::open_in_memory().unwrap(),
    )));
    let registry = SourceRegistryService::new(repos);
    let source_id = registry
        .add_custom_source("Directory", &format!("{}/opds/", server.origin()))
        .await
        .unwrap()
        .source_id;
    let client = OpdsClient::new()
        .unwrap()
        .with_source_registry(registry.clone());
    for (index, base) in ["/opds/", "/library/"].into_iter().enumerate() {
        let endpoint = format!("{}{base}", server.origin());
        if index > 0 {
            registry
                .update_custom_source(haven_application::wire::SourceUpdateRequest {
                    source_id: source_id.clone(),
                    display_name: None,
                    endpoint: Some(endpoint.clone()),
                })
                .await
                .unwrap();
        }
        let stored = registry.endpoint(&source_id).await.unwrap().unwrap();
        assert_eq!(stored, endpoint);
        let entries = client
            .search_entries(Some(&source_id), &stored, "book", 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
    }
    assert_eq!(
        server.paths(),
        vec![
            "/opds/",
            "/opds/search.xml",
            "/opds/results",
            "/library/",
            "/library/search.xml",
            "/library/results"
        ]
    );
}

#[tokio::test]
async fn long_unicode_summaries_keep_search_results_within_the_byte_limit() {
    let server = FixtureServer::start(|_| {
        vec![
            ("/opds".into(), FixtureResponse::atom(root_feed())),
            (
                "/opensearch.xml".into(),
                FixtureResponse::opensearch(
                    r#"<OpenSearchDescription><Url type="application/atom+xml" template="/results?q={searchTerms}"/></OpenSearchDescription>"#,
                ),
            ),
            (
                "/results".into(),
                FixtureResponse::atom(format!(
                    r#"<feed><entry><id>urn:chinese</id><title>中文</title><summary>{}</summary></entry><entry><id>urn:emoji</id><title>Emoji</title><summary>{}</summary></entry><entry><id>urn:ascii</id><title>ASCII</title><summary>{}</summary></entry></feed>"#,
                    "书".repeat(700),
                    "📚".repeat(501),
                    "a".repeat(2100)
                )),
            ),
        ]
    });
    let endpoint = format!("{}/opds", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new().unwrap().with_source_registry(registry);
    let entries = client
        .search_entries(Some(&source_id), &endpoint, "book", 10)
        .await
        .unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0].summary.as_deref(),
        Some("书".repeat(666).as_str())
    );
    assert_eq!(
        entries[1].summary.as_deref(),
        Some("📚".repeat(500).as_str())
    );
    assert_eq!(entries[2].summary.as_ref().unwrap().len(), 2000);
}

#[tokio::test]
async fn xml_link_attributes_are_decoded_once_for_search_and_artwork() {
    let server = FixtureServer::start(|_| {
        vec![
            ("/opds".into(), FixtureResponse::atom(root_feed())),
            (
                "/opensearch.xml".into(),
                FixtureResponse::opensearch(
                    r#"<OpenSearchDescription><Url type="application/atom+xml" template="/results?kind=book&amp;q={searchTerms}"/></OpenSearchDescription>"#,
                ),
            ),
            (
                "/results".into(),
                FixtureResponse::atom(
                    r#"<feed><entry><id>urn:book</id><title>Book</title><link rel="http://opds-spec.org/image" type="image/png" href="/cover?size=small&amp;id=1&amp;amp;literal=yes"/></entry></feed>"#,
                ),
            ),
        ]
    });
    let endpoint = format!("{}/opds", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new().unwrap().with_source_registry(registry);
    for query in ["first", "second"] {
        let entries = client
            .search_entries(Some(&source_id), &endpoint, query, 10)
            .await
            .unwrap();
        assert_eq!(
            entries[0].pic,
            Some(format!(
                "{}/cover?size=small&id=1&amp;literal=yes",
                server.origin()
            ))
        );
    }
    assert_eq!(
        server
            .requests()
            .iter()
            .map(|r| r.target())
            .collect::<Vec<_>>(),
        vec![
            "/opds",
            "/opensearch.xml",
            "/results?kind=book&q=first",
            "/results?kind=book&q=second"
        ]
    );
}

#[tokio::test]
async fn redirects_preserve_the_final_base_for_discovery_results_and_entry_links() {
    let server = FixtureServer::start(|_| {
        vec![
            (
                "/opds".into(),
                FixtureResponse::redirect("/catalog/index.xml"),
            ),
            (
                "/catalog/index.xml".into(),
                FixtureResponse::atom(r#"<feed><link rel="search" href="search.xml"/></feed>"#),
            ),
            (
                "/catalog/search.xml".into(),
                FixtureResponse::redirect("/discovery/description.xml"),
            ),
            (
                "/discovery/description.xml".into(),
                FixtureResponse::opensearch(
                    r#"<OpenSearchDescription><Url type="application/atom+xml" template="results?q={searchTerms}"/></OpenSearchDescription>"#,
                ),
            ),
            (
                "/discovery/results".into(),
                FixtureResponse::redirect("/final/feed.xml"),
            ),
            (
                "/final/feed.xml".into(),
                FixtureResponse::atom(
                    r#"<feed><entry><id>urn:book</id><title>Book</title><link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="book.epub"/><link rel="http://opds-spec.org/image" type="image/png" href="cover.png"/></entry></feed>"#,
                ),
            ),
            (
                "/entry".into(),
                FixtureResponse::redirect("/final/feed.xml"),
            ),
        ]
    });
    let endpoint = format!("{}/opds", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new().unwrap().with_source_registry(registry);
    for query in ["first", "second"] {
        let entries = client
            .search_entries(Some(&source_id), &endpoint, query, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].epub_href,
            Some(format!("{}/final/book.epub", server.origin()))
        );
        assert_eq!(
            entries[0].pic,
            Some(format!("{}/final/cover.png", server.origin()))
        );
    }
    let entry = client
        .fetch_entry_metadata(Some(&source_id), &format!("{}/entry", server.origin()))
        .await
        .unwrap();
    assert_eq!(
        entry.epub_href,
        Some(format!("{}/final/book.epub", server.origin()))
    );
    assert_eq!(
        server.paths(),
        vec![
            "/opds",
            "/catalog/index.xml",
            "/catalog/search.xml",
            "/discovery/description.xml",
            "/discovery/results",
            "/final/feed.xml",
            "/discovery/results",
            "/final/feed.xml",
            "/entry",
            "/final/feed.xml"
        ]
    );
}

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

/// 根目录在 `/opds/root.xml`，`rel="search"` 指向另一个目录里的描述文件。
fn relative_template_root_feed() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:haven:fixture:relative-root</id>
  <title>自建书库</title>
  <link rel="search" type="application/atom+xml" href="/discovery/description.xml"/>
</feed>"#
        .to_owned()
}

/// 模板是**相对描述文件**的路径：只有以描述文件地址为基准解析才会落到
/// `/discovery/results.opds`；若改用登记端点（`/opds/root.xml`）为基准，就会错落到
/// `/opds/results.opds`。
fn relative_opensearch_description() -> String {
    r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>自建书库</ShortName>
  <Url type="application/atom+xml" template="results.opds?q={searchTerms}"/>
</OpenSearchDescription>"#
        .to_owned()
}

fn relative_template_server() -> FixtureServer {
    FixtureServer::start(|_| {
        vec![
            (
                "/opds/root.xml".to_owned(),
                FixtureResponse::atom(relative_template_root_feed()),
            ),
            (
                "/discovery/description.xml".to_owned(),
                FixtureResponse::opensearch(relative_opensearch_description()),
            ),
            (
                "/discovery/results.opds".to_owned(),
                FixtureResponse::atom(
                    r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry><id>urn:uuid:relative-1</id><title>Relative Result</title></entry>
</feed>"#,
                ),
            ),
        ]
    })
}

/// 二次搜索命中缓存模板时，仍以**发现模板的描述文件**为相对基准。
///
/// 旧实现只缓存原始模板文本，并用登记端点作 base：相对模板 `results.opds?...` 会被解析到
/// `/opds/results.opds`（或直接超出权威范围），第二次搜索要么失败、要么落到错误地址。
#[tokio::test]
async fn cached_search_template_keeps_the_descriptor_as_its_base() {
    let server = relative_template_server();
    let endpoint = format!("{}/opds/root.xml", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new()
        .unwrap()
        .with_loopback_fixture()
        .with_source_registry(registry);

    let first = client
        .search_url(Some(&source_id), &endpoint, "first")
        .await
        .unwrap();
    let second = client
        .search_url(Some(&source_id), &endpoint, "second")
        .await
        .unwrap();

    assert_eq!(
        first,
        format!("{}/discovery/results.opds?q=first", server.origin())
    );
    assert_eq!(
        second,
        format!("{}/discovery/results.opds?q=second", server.origin())
    );
    assert_eq!(
        server.paths(),
        vec!["/opds/root.xml", "/discovery/description.xml"],
        "缓存命中不得再访问根目录或描述文件"
    );
}

/// 公开消费者 `search_entries` 连搜两次：第二次只请求缓存模板解析出的结果地址。
#[tokio::test]
async fn search_entries_reuses_the_descriptor_base_across_searches() {
    let server = relative_template_server();
    let endpoint = format!("{}/opds/root.xml", server.origin());
    let (registry, source_id) = seeded_opds_registry(&endpoint, false).await;
    let client = OpdsClient::new()
        .unwrap()
        .with_loopback_fixture()
        .with_source_registry(registry);

    let first = client
        .search_entries(Some(&source_id), &endpoint, "first", 10)
        .await
        .unwrap();
    let second = client
        .search_entries(Some(&source_id), &endpoint, "second", 10)
        .await
        .unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].title, "Relative Result");
    assert_eq!(second[0].title, "Relative Result");
    assert_eq!(
        server
            .requests()
            .iter()
            .map(|request| request.target())
            .collect::<Vec<_>>(),
        vec![
            "/opds/root.xml",
            "/discovery/description.xml",
            "/discovery/results.opds?q=first",
            "/discovery/results.opds?q=second",
        ],
        "第二次搜索只复用缓存的 (模板, 描述文件地址)，不再走根目录与描述文件"
    );
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
fn import_enabled_gutenberg_requires_an_http_entry_identity() {
    let client = OpdsClient::new().unwrap();
    let context = super::OpdsRequestContext::builtin_gutenberg(super::OPDS_SOURCE_GUTENBERG);
    let endpoint = "https://www.gutenberg.org/ebooks/";
    let mut entry = super::OpdsEntry {
        entry_id: "urn:uuid:book-1".to_owned(),
        title: "Book".to_owned(),
        author: None,
        summary: None,
        pic: None,
        epub_href: Some("https://www.gutenberg.org/ebooks/84.epub".to_owned()),
    };
    assert!(!client.candidate_stays_on_source(&context, endpoint, &entry));
    entry.entry_id = "https://www.gutenberg.org/ebooks/84.opds".to_owned();
    assert!(client.candidate_stays_on_source(&context, endpoint, &entry));
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
