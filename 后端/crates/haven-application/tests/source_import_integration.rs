//! V02-REMOTE-IMPORT-SEPARATION-001 集成回归。
//!
//! 搜索/详情导入的唯一职责是建立 Work、Edition、MediaItem 与远端
//! `SourceObject` 身份。它不能偷偷获取正文、创建本地文件或产生 Offline
//! Resource；只有后续显式 `download_create` 流程才允许落盘。

use std::fs;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_application::services::ports::SourceImportPorts;
use haven_application::services::source_import::{
    ImportedWork, RemoteContentRef, SourceCatalogEntry, SourceCatalogProvider, SourceImportService,
};
use haven_application::services::source_registry::SourceRegistryService;
use haven_common::AppError;
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ResourceRepository, WorkRepository,
};
use haven_domain::entities::ResourceLocator;
use haven_domain::enums::{MediaType, ResourceType};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::SqliteRepositories;

const MANGA_ID: &str = "00000000-0000-0000-0000-000000000001";
const MANGA_CHAPTER_ID: &str = "00000000-0000-0000-0000-000000000002";
const MANGA_REMOTE_ID: &str =
    "00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002";
const ARXIV_ID: &str = "hep-th/9901001";
const PMCID: &str = "PMC123456";
const WIKISOURCE_TITLE: &str = "三国演义/第一回";
const M3U_DEDUPE_KEY: &str = "Fixture Stream\u{1}https://cdn.example.invalid/hls/master.m3u8";
const M3U_CANDIDATE: &str = "content-candidate-m3u-Fixture%20Stream%01https%3A%2F%2Fcdn.example.invalid%2Fhls%2Fmaster.m3u8";

#[derive(Default)]
struct FakeCatalog {
    detail_calls: Mutex<Vec<(String, String, String)>>,
}

impl FakeCatalog {
    fn detail_count(&self) -> usize {
        self.detail_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[async_trait]
impl SourceCatalogProvider for FakeCatalog {
    async fn detail(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        self.detail_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((
                source_id.to_owned(),
                endpoint.to_owned(),
                external_id.to_owned(),
            ));

        let (media_type, mime_type) = match source_id {
            "mangadex" => (MediaType::Comic, "application/vnd.comicbook+zip"),
            "arxiv" => (MediaType::Article, "application/pdf"),
            "europepmc" | "wikisource" => (MediaType::Article, "text/html; charset=utf-8"),
            "opds_gutenberg" => (MediaType::Book, "application/epub+zip"),
            _ => {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    haven_common::ErrorKind::Validation,
                    "测试来源不存在",
                    false,
                ));
            }
        };

        let remote_id = if source_id == "mangadex" {
            format!("{external_id}:{MANGA_CHAPTER_ID}")
        } else {
            external_id.to_owned()
        };
        Ok(SourceCatalogEntry {
            external_id: external_id.to_owned(),
            title: format!("测试条目 {external_id}"),
            year: Some(2026),
            type_name: Some("测试正文".to_owned()),
            pic: None,
            episodes: Vec::new(),
            content: Some("测试元数据，不代表真实正文".to_owned()),
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(media_type),
            remote: Some(RemoteContentRef {
                source_key: source_id.to_owned(),
                remote_id,
                media_type,
                mime_type: Some(mime_type.to_owned()),
            }),
        })
    }
}

struct Fixture {
    db: Arc<Db>,
    service: SourceImportService,
    repos: Arc<SqliteRepositories>,
    registry: SourceRegistryService,
    catalog: Arc<FakeCatalog>,
}

fn fixture() -> Fixture {
    let db = Arc::new(Db::open_in_memory().expect("in-memory DB should open"));
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let import_ports: Arc<dyn SourceImportPorts> = repos.clone();
    let registry = SourceRegistryService::new(repos.clone());
    let catalog = Arc::new(FakeCatalog::default());
    let service = SourceImportService::new(
        import_ports,
        Arc::new(haven_infrastructure::db::uow::SqliteUnitOfWork::new(
            db.clone(),
        )),
        registry.clone(),
        catalog.clone(),
    );
    Fixture {
        db,
        service,
        repos,
        registry,
        catalog,
    }
}

async fn assert_remote_resource(
    repos: &SqliteRepositories,
    imported: &ImportedWork,
    source_key: &str,
    remote_id: &str,
) {
    let editions = repos
        .list_by_work(imported.work_id)
        .await
        .expect("imported work should have an edition");
    assert_eq!(editions.len(), 1);
    let items = repos
        .list_by_edition(editions[0].id)
        .await
        .expect("imported edition should have a media item");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, imported.media_item_id);

    let resources = repos
        .list_by_media_item(imported.media_item_id)
        .await
        .expect("imported media item should have a resource");
    assert_eq!(resources.len(), 1, "导入只应建立一个远端资源");
    let resource = &resources[0];
    assert!(
        resource.storage_location_id.is_none(),
        "导入阶段不得登记本地存储位置"
    );
    assert!(resource.size.is_none(), "导入阶段不得伪造正文大小");
    match &resource.locator {
        ResourceLocator::SourceObject {
            source_id: _,
            remote_id: stored_remote_id,
        } => assert_eq!(stored_remote_id, remote_id),
        other => panic!("导入必须写入 SourceObject，实际为 {other:?}"),
    }
    assert_eq!(
        resource.source_id,
        Some(
            haven_application::services::source_import::stable_source_id(source_key)
                .expect("allowlisted source")
        )
    );
}

async fn assert_m3u_resource(repos: &SqliteRepositories, imported: &ImportedWork) {
    let editions = repos
        .list_by_work(imported.work_id)
        .await
        .expect("M3U work should have an edition");
    assert_eq!(editions.len(), 1);
    assert_eq!(editions[0].edition_type, MediaType::Series);

    let items = repos
        .list_by_edition(editions[0].id)
        .await
        .expect("M3U edition should have a media item");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, imported.media_item_id);
    assert_eq!(items[0].media_type, MediaType::Episode);

    let resources = repos
        .list_by_media_item(imported.media_item_id)
        .await
        .expect("M3U media item should have a resource");
    assert_eq!(resources.len(), 1);
    let resource = &resources[0];
    assert_eq!(resource.resource_type, ResourceType::HlsStream);
    assert_eq!(
        resource.mime_type.as_deref(),
        Some("application/vnd.apple.mpegurl")
    );
    assert_eq!(
        resource.locator,
        ResourceLocator::Http {
            url: "https://cdn.example.invalid/hls/master.m3u8".to_owned(),
        }
    );
}

fn table_count(db: &Db, table: &str) -> i64 {
    db.with_tx(|conn| {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(|error| {
            AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                format!("读取 {table} 测试计数失败"),
                false,
            )
            .with_source(error)
        })
    })
    .expect("test table count should succeed")
}

#[tokio::test]
async fn content_import_registers_source_object_without_local_content() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("temporary directory should open");

    let imported = fixture
        .service
        .import_content_candidate("mangadex", MANGA_ID)
        .await
        .expect("valid content candidate should import");

    assert_remote_resource(&fixture.repos, &imported, "mangadex", MANGA_REMOTE_ID).await;
    assert_eq!(fixture.catalog.detail_count(), 1);
    assert!(
        fs::read_dir(temp.path())
            .expect("temporary directory should read")
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn m3u_import_persists_a_playable_chain_and_is_idempotent() {
    let fixture = fixture();

    let first = fixture
        .service
        .import_candidate(M3U_CANDIDATE)
        .await
        .expect("valid M3U candidate should import");
    let second = fixture
        .service
        .import_candidate(M3U_CANDIDATE)
        .await
        .expect("repeated M3U candidate should be idempotent");

    assert_eq!(first, second);
    assert_eq!(
        fixture
            .repos
            .id_for_source_ref("m3u", M3U_DEDUPE_KEY)
            .await
            .expect("M3U source ref lookup should succeed"),
        Some(first.work_id)
    );
    assert_eq!(fixture.repos.list(100, 0).await.unwrap().len(), 1);
    assert_eq!(
        fixture.catalog.detail_count(),
        0,
        "M3U import must not refetch playlist detail"
    );
    assert_m3u_resource(&fixture.repos, &first).await;
}

#[tokio::test]
async fn m3u_resource_failure_rolls_back_the_entire_import_chain() {
    let fixture = fixture();
    fixture
        .db
        .with_tx(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_m3u_resource_insert
                 BEFORE INSERT ON resources
                 BEGIN
                   SELECT RAISE(ABORT, 'injected M3U resource failure');
                 END;",
            )
            .map_err(|error| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "安装 M3U 资源失败注入器失败",
                    false,
                )
                .with_source(error)
            })?;
            Ok(())
        })
        .expect("resource failure trigger should install");

    let error = fixture
        .service
        .import_candidate(M3U_CANDIDATE)
        .await
        .expect_err("injected resource failure should fail the import");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");

    assert_eq!(
        fixture
            .repos
            .id_for_source_ref("m3u", M3U_DEDUPE_KEY)
            .await
            .expect("M3U source ref lookup should succeed after rollback"),
        None
    );
    for table in [
        "works",
        "work_source_refs",
        "editions",
        "media_items",
        "resources",
    ] {
        assert_eq!(table_count(&fixture.db, table), 0, "{table} must roll back");
    }
}

#[tokio::test]
async fn repeated_content_import_is_idempotent_and_does_not_repeat_detail_fetch() {
    let fixture = fixture();

    let first = fixture
        .service
        .import_content_candidate("arxiv", ARXIV_ID)
        .await
        .expect("first import should succeed");
    let second = fixture
        .service
        .import_content_candidate("arxiv", ARXIV_ID)
        .await
        .expect("second import should be idempotent");

    assert_eq!(first, second);
    assert_eq!(
        fixture.catalog.detail_count(),
        1,
        "重复导入应命中来源引用去重"
    );
    assert_remote_resource(&fixture.repos, &first, "arxiv", ARXIV_ID).await;
}

#[tokio::test]
async fn all_fixed_content_sources_keep_remote_identity_during_import() {
    for (source_key, remote_id) in [
        ("mangadex", MANGA_ID),
        ("arxiv", ARXIV_ID),
        ("europepmc", PMCID),
        ("wikisource", WIKISOURCE_TITLE),
    ] {
        let fixture = fixture();
        let imported = fixture
            .service
            .import_content_candidate(source_key, remote_id)
            .await
            .expect("fixed source candidate should import");
        let expected_remote_id = if source_key == "mangadex" {
            MANGA_REMOTE_ID
        } else {
            remote_id
        };
        assert_remote_resource(&fixture.repos, &imported, source_key, expected_remote_id).await;
    }
}

#[tokio::test]
async fn opds_import_does_not_fetch_or_create_an_epub() {
    let fixture = fixture();
    fixture
        .registry
        .list()
        .await
        .expect("registry should seed the built-in Gutenberg endpoint");
    let temp = tempfile::tempdir().expect("temporary directory should open");
    let entry_url = "https://www.gutenberg.org/ebooks/84.epub3.images";

    let imported = fixture
        .service
        .import_opds_candidate("opds_gutenberg", entry_url)
        .await
        .expect("Gutenberg OPDS candidate should import metadata");

    assert_remote_resource(&fixture.repos, &imported, "opds_gutenberg", entry_url).await;
    assert_eq!(fixture.catalog.detail_count(), 1);
    assert!(
        fs::read_dir(temp.path())
            .expect("temporary directory should read")
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn unsupported_custom_opds_import_is_rejected_before_network_access() {
    let fixture = fixture();
    let err = fixture
        .service
        .import_opds_candidate("custom_example", "https://example.invalid/book.opds")
        .await
        .expect_err("custom OPDS has no controlled acquisition provider");

    assert_eq!(err.code().as_str(), "SOURCE_IMPORT_UNSUPPORTED");
    assert_eq!(
        fixture.catalog.detail_count(),
        0,
        "拒绝必须发生在网络访问前"
    );
}

#[tokio::test]
async fn invalid_source_and_provider_ids_are_rejected_before_catalog_detail() {
    let fixture = fixture();
    for (source_key, remote_id) in [
        ("unknown", MANGA_ID),
        ("mangadex", "https://evil.invalid/manga"),
        ("arxiv", "https://evil.invalid/paper.pdf"),
        ("europepmc", "PMCX"),
        ("wikisource", "https://evil.invalid/page"),
    ] {
        let err = fixture
            .service
            .import_content_candidate(source_key, remote_id)
            .await
            .expect_err("invalid remote identity should fail closed");
        assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    }
    assert_eq!(
        fixture.catalog.detail_count(),
        0,
        "非法标识不得触发详情请求"
    );
}

#[tokio::test]
async fn opaque_candidate_handle_routes_to_content_import_without_exposing_a_url() {
    let fixture = fixture();
    let handle = format!("content-candidate-mangadex-{MANGA_ID}");
    let imported = fixture
        .service
        .import_candidate(&handle)
        .await
        .expect("opaque content candidate should route through the application");
    assert_remote_resource(&fixture.repos, &imported, "mangadex", MANGA_REMOTE_ID).await;
}

#[tokio::test]
async fn metadata_only_candidate_is_rejected_without_falling_back_to_cms10() {
    let fixture = fixture();

    let err = fixture
        .service
        .import_candidate("metadata-candidate-gutenberg-deadbeef")
        .await
        .expect_err("metadata-only candidates must not be imported");

    assert_eq!(err.code().as_str(), "SOURCE_IMPORT_UNSUPPORTED");
    assert_eq!(
        fixture.catalog.detail_count(),
        0,
        "metadata-only rejection must happen before any Provider detail call"
    );
}

#[tokio::test]
async fn candidate_without_an_opaque_prefix_is_rejected() {
    let fixture = fixture();

    let err = fixture
        .service
        .import_candidate("raw-cms10-id")
        .await
        .expect_err("raw external IDs must not be routed implicitly");

    assert_eq!(err.code().as_str(), "SOURCE_IMPORT_UNSUPPORTED");
    assert_eq!(fixture.catalog.detail_count(), 0);
}
