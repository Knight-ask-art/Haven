//! 自托管漫画库（Komga/Kavita）生产路由集成测试。
//!
//! 证明的是组合根真正使用的那条链路，而不只是 Provider 单测：
//! `ComicLibrarySearchParticipant` 搜索 → opaque 候选 → `SourceCatalogProvider`
//! 详情（含章节目录）→ `RemoteComicPageProvider` 逐页读取 → 通过
//! `RoutingRemoteAcquisitionPort` 的受控离线获取（CBZ）。
//!
//! 网络收在 `ComicLibraryGateway` 端口后面，因此这里不需要真实 Komga/Kavita
//! 服务器；真实服务端往返与桌面构建仍属于单独验收项。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_application::services::comic::{PreparedComicPageSource, RemoteComicPageProvider};
use haven_application::services::ports::{RemoteAcquiredFile, RemoteAcquisitionPort};
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::session::{PreparedSession, PreparedSessionSource};
use haven_application::services::source_import::{
    KOMGA_SOURCE_KEY, SourceCatalogProvider, comic_library_work_ref, stable_source_id,
};
use haven_application::services::source_registry::{CustomSourceKind, SourceRegistryService};
use haven_common::{AppError, ErrorKind};
use haven_domain::enums::{MediaType, ResourceType};
use haven_infrastructure::Db;
use haven_infrastructure::comic_library_sources::{
    ApiKeyResolver, ComicLibraryGateway, ComicLibraryProvider, ComicLibrarySearchParticipant,
    LibraryMethod, LibraryRequest, LibraryResponse,
};
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::opds::{OpdsCatalogProvider, OpdsClient, RoutingRemoteAcquisitionPort};

const SERIES_LIST: &str = r#"{
  "content": [{"id": "series-a", "name": "文件夹", "metadata": {"title": "测试漫画"}}],
  "totalElements": 1, "totalPages": 1, "number": 0, "size": 10
}"#;

const SERIES_DETAIL: &str =
    r#"{"id": "series-a", "name": "文件夹", "metadata": {"title": "测试漫画", "summary": "简介"}}"#;

const BOOKS: &str = r#"{
  "content": [{"id": "book-1", "seriesId": "series-a", "metadata": {"number": "1", "numberSort": 1.0}, "media": {"pagesCount": 2}}],
  "totalElements": 1, "totalPages": 1, "number": 0, "size": 100
}"#;

const PAGES: &str = r#"[{"number": 1}, {"number": 2}]"#;

fn png(payload: &[u8]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

/// 记录请求的进程内 gateway。断言点之一是「API key 只出现在请求头字段里」。
struct FixtureGateway {
    bodies: Mutex<HashMap<String, Vec<u8>>>,
    requests: Mutex<Vec<LibraryRequest>>,
}

impl FixtureGateway {
    fn new() -> Arc<Self> {
        let mut bodies: HashMap<String, Vec<u8>> = HashMap::new();
        bodies.insert(
            "/api/v1/series/list".into(),
            SERIES_LIST.as_bytes().to_vec(),
        );
        bodies.insert(
            "/api/v1/series/series-a".into(),
            SERIES_DETAIL.as_bytes().to_vec(),
        );
        bodies.insert("/api/v1/books/list".into(), BOOKS.as_bytes().to_vec());
        bodies.insert(
            "/api/v1/books/book-1/pages".into(),
            PAGES.as_bytes().to_vec(),
        );
        bodies.insert("/api/v1/books/book-1/pages/1/raw".into(), png(b"page-one"));
        bodies.insert("/api/v1/books/book-1/pages/2/raw".into(), png(b"page-two"));
        // 归档端点故意返回非 ZIP：Provider 必须回退到逐页组装出合法 CBZ。
        bodies.insert(
            "/api/v1/books/book-1/file".into(),
            b"Rar!\x1a\x07\x00not-a-zip".to_vec(),
        );
        Arc::new(Self {
            bodies: Mutex::new(bodies),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<LibraryRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[async_trait]
impl ComicLibraryGateway for FixtureGateway {
    async fn send(
        &self,
        request: LibraryRequest,
        _max_bytes: usize,
    ) -> Result<LibraryResponse, AppError> {
        let body = {
            let bodies = self
                .bodies
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Fixture 只按 path 路由；分页和搜索 query 仍保留在已记录的原始请求中供断言。
            let path = request.path.split('?').next().unwrap_or_default();
            bodies.get(path).cloned()
        };
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(request);
        body.map(|bytes| LibraryResponse { bytes }).ok_or_else(|| {
            AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "fixture 未配置该路径",
                true,
            )
        })
    }
}

/// 只用于 `RoutingRemoteAcquisitionPort` 的固定来源分支；漫画库永远不该走到它。
struct NeverAcquire;

#[async_trait]
impl RemoteAcquisitionPort for NeverAcquire {
    async fn acquire(
        &self,
        _source_key: &str,
        _remote_id: &str,
        _destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        Err(AppError::new(
            "SOURCE_IMPORT_UNSUPPORTED",
            ErrorKind::Unsupported,
            "固定来源分支不应被漫画库命中",
            false,
        ))
    }
}

fn registry() -> SourceRegistryService {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    SourceRegistryService::new(repos)
}

#[tokio::test]
async fn komga_search_import_catalog_page_read_and_offline_download_route() {
    let registry = registry();
    let source_id = registry
        .add_comic_library_source(
            "我的 Komga",
            "https://komga.example.invalid",
            CustomSourceKind::Komga,
        )
        .await
        .unwrap()
        .source_id;
    let gateway = FixtureGateway::new();
    let api_key: Arc<ApiKeyResolver> =
        Arc::new(|_source_id: &str| Box::pin(async move { Some("komga-api-key".to_owned()) }));
    let provider = Arc::new(ComicLibraryProvider::new(
        registry.clone(),
        gateway.clone(),
        api_key.clone(),
    ));

    // 1. 搜索：候选句柄必须 opaque，且不带地址或 API key。
    let participant = ComicLibrarySearchParticipant::new(
        registry.clone(),
        gateway.clone(),
        api_key.clone(),
        "custom_komga_",
    );
    let not_cancelled = || false;
    let cards = participant
        .search_for(&source_id, "测试", 10, &not_cancelled)
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert!(
        !cards[0].work_id.contains("komga.example.invalid")
            && !cards[0].work_id.contains("komga-api-key"),
        "候选句柄泄漏了地址或凭据: {}",
        cards[0].work_id
    );

    // 2. 详情（含章节目录）：远端身份必须是章节级 opaque 身份。
    let work_ref = comic_library_work_ref(&source_id, "series-a").unwrap();
    let entry = provider.detail(&source_id, "", &work_ref).await.unwrap();
    assert_eq!(entry.title, "测试漫画");
    let remote = entry.remote.clone().expect("必须给出可导入的远端身份");
    assert_eq!(remote.source_key, KOMGA_SOURCE_KEY);
    assert_eq!(remote.remote_id, format!("{work_ref}:book-1"));
    let catalog = entry.comic_catalog.clone().expect("详情必须带章节目录");
    assert_eq!(catalog.readable_chapters().count(), 1);

    // 3. 在线逐页读取：页身份来自服务端页清单，不接受前端任意页名。
    let session = PreparedSession {
        work_id: "work".into(),
        edition_id: "edition".into(),
        media_item_id: uuid::Uuid::now_v7().to_string(),
        engine: haven_application::wire::SessionEngineDto::Comic,
        resource_id: haven_domain::ids::ResourceId::new(),
        storage_location_id: None,
        canonical_root: None,
        canonical_file: None,
        subtitle_tracks: Vec::new(),
        source: PreparedSessionSource::Remote {
            source_id: stable_source_id(KOMGA_SOURCE_KEY).unwrap(),
            source_key: KOMGA_SOURCE_KEY.to_owned(),
            remote_id: remote.remote_id.clone(),
        },
        mime_type: Some("application/vnd.comicbook+zip".to_owned()),
        media_type: MediaType::Comic,
        resource_type: ResourceType::ComicArchive,
        comic_pages: None,
        progress: None,
    };
    let pages = provider.inspect(&session).await.unwrap();
    assert_eq!(pages.len(), 2);
    let first = provider.read_page(&session, &pages[0]).await.unwrap();
    assert_eq!(
        first.mime_type,
        haven_application::services::comic::ComicImageMime::Png
    );
    assert_eq!(first.bytes, png(b"page-one"));

    // 伪造页名必须 fail closed，而不是去读一个前端指定的页。
    let forged = haven_application::services::comic::PreparedComicPage {
        availability: haven_application::services::comic::PreparedComicPageAvailability::Ready,
        identity: pages[0].identity.clone(),
        source: PreparedComicPageSource::RemotePage {
            page_name: "../etc/passwd".to_owned(),
        },
    };
    assert!(provider.read_page(&session, &forged).await.is_err());

    // 4. 离线下载：走生产路由器，归档端点返回非 ZIP 时必须回退并产出合法 CBZ。
    let opds = Arc::new(OpdsCatalogProvider::new(Arc::new(
        OpdsClient::new().unwrap(),
    )));
    let acquisition = RoutingRemoteAcquisitionPort::new(opds, Arc::new(NeverAcquire))
        .with_comic_library(provider.clone());
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("chapter.cbz");
    let acquired = acquisition
        .acquire(KOMGA_SOURCE_KEY, &remote.remote_id, &destination)
        .await
        .unwrap();
    assert_eq!(acquired.mime, "application/vnd.comicbook+zip");
    assert_eq!(
        acquired.size_bytes,
        std::fs::metadata(&destination).unwrap().len()
    );

    // 非 ZIP 服务端响应走逐页组装；这里核对生成归档的 ZIP 文件头与页名。
    let bytes = std::fs::read(&destination).unwrap();
    assert!(
        bytes.starts_with(b"PK\x03\x04"),
        "下载对象必须是 ZIP/CBZ（服务端返回的 CBR 不得被直接写成 CBZ）"
    );
    // CBZ 由本地逐页组装：两个页名都以未压缩条目名出现在归档里。
    let raw = String::from_utf8_lossy(&bytes);
    assert!(raw.contains("page-0001.png"));
    assert!(raw.contains("page-0002.png"));

    // 5. 凭据只出现在请求头字段里，绝不进入 endpoint/path。
    let requests = gateway.requests();
    assert!(!requests.is_empty());
    for request in &requests {
        assert!(
            !request.path.contains("komga-api-key"),
            "API key 不得进入请求路径: {}",
            request.path
        );
        assert!(!request.endpoint.contains("komga-api-key"));
        assert!(
            !request.path.contains("komga.example.invalid"),
            "path 只能是固定 API 路径: {}",
            request.path
        );
    }
    assert!(
        requests
            .iter()
            .any(|request| request.api_key.as_deref() == Some("komga-api-key")),
        "API key 必须经请求字段注入"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == LibraryMethod::Get
                && request.api_key_header == "X-API-Key"),
        "Komga 必须使用 X-API-Key 请求头"
    );
}

#[tokio::test]
async fn comic_library_download_is_rejected_without_a_wired_provider() {
    let opds = Arc::new(OpdsCatalogProvider::new(Arc::new(
        OpdsClient::new().unwrap(),
    )));
    let acquisition = RoutingRemoteAcquisitionPort::new(opds, Arc::new(NeverAcquire));
    let directory = tempfile::tempdir().unwrap();
    let error = acquisition
        .acquire(
            KOMGA_SOURCE_KEY,
            "custom_komga_0123456789ab:series-a:book-1",
            &directory.path().join("chapter.cbz"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    assert!(error.retryable());
}

#[tokio::test]
async fn comic_library_search_never_touches_another_source_family() {
    let registry = registry();
    let gateway = FixtureGateway::new();
    let api_key: Arc<ApiKeyResolver> = Arc::new(|_source_id: &str| Box::pin(async move { None }));
    let participant =
        ComicLibrarySearchParticipant::new(registry, gateway.clone(), api_key, "custom_komga_");
    let not_cancelled = || false;
    // 订阅源与内置来源都不该被漫画库参与者接管，也不会触发任何请求。
    for foreign in [
        "custom_feed_0123456789ab",
        "custom_0123456789ab",
        "mangadex",
    ] {
        assert!(
            participant
                .search_for(foreign, "任意", 10, &not_cancelled)
                .await
                .unwrap()
                .is_empty(),
            "{foreign} 不应被漫画库参与者接管"
        );
    }
    assert!(gateway.requests().is_empty());
}
