//! 报刊（Europe PMC）来源导入的分层集成证据。
//!
//! 覆盖：期刊 → 卷 → 期 → 文章 → MediaItem → Resource 的真实归属、重复导入
//! 幂等（不再请求 Provider）、metadata-only 不被宣称可读、失败事务不留下
//! source_ref 或孤儿层级，以及缺少 ISSN 时的诚实回退。
//!
//! Provider 使用本地 fake，不访问网络；Sqlite 走真实 Repository 与真实
//! UnitOfWork，因此这里验证的是实际的写入与回滚语义。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use haven_application::services::periodical::{
    EUROPE_PMC_SOURCE_KEY, PeriodicalArticleAvailability, PeriodicalArticleRecord,
    PeriodicalIssueRecord, PeriodicalJournalRecord, PeriodicalProvider, PeriodicalVolumeRecord,
};
use haven_application::services::ports::{
    FavoriteTxPorts, PeriodicalImportPlan, RemoteAcquisitionPort, RemoteSessionPort,
    SourceImportPorts, SourceRegistryPorts, UnitOfWork,
};
use haven_application::services::search_source::{SearchEventSink, SearchSourceParticipant};
use haven_application::services::source_import::{
    FEED_SOURCE_KEY, ImportedWork, SourceCatalogEntry, SourceCatalogProvider, SourceImportService,
    feed_entry_digest,
};
use haven_application::services::source_registry::SourceRegistryService;
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, PeriodicalRepository, ResourceRepository,
    WorkRepository,
};
use haven_domain::entities::{Edition, MediaItem, Resource, Work};
use haven_domain::enums::{Availability, MediaItemStatus, MediaType, ResourceType};
use haven_domain::ids::MediaItemId;
use haven_domain::periodical::{Doi, Issn, PageRange};
use haven_infrastructure::Db;
use haven_infrastructure::article_feeds::{FeedProvider, FeedSearchParticipant, FeedSource};
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::db::uow::SqliteUnitOfWork;

const PMCID_A: &str = "PMC1234567";
const PMCID_B: &str = "PMC7654321";
const PMCID_C: &str = "PMC1111111";

struct FakePeriodicalProvider {
    records: Mutex<Vec<PeriodicalArticleRecord>>,
    calls: AtomicUsize,
}

impl FakePeriodicalProvider {
    fn new(records: Vec<PeriodicalArticleRecord>) -> Self {
        Self {
            records: Mutex::new(records),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PeriodicalProvider for FakePeriodicalProvider {
    async fn article(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<PeriodicalArticleRecord, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if source_key != EUROPE_PMC_SOURCE_KEY {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "该来源不是 Europe PMC",
                false,
            ));
        }
        self.records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|record| record.remote_article_id == remote_article_id)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    "SOURCE_UNAVAILABLE",
                    ErrorKind::Network,
                    "fake provider 没有该条目",
                    true,
                )
            })
    }
}

/// 报刊路径在 Provider/Repository 齐备时不得回退到通用目录；该 stub 用唯一
/// 错误码证明请求确实走到了通用路径。
struct LegacyCatalogStub;

#[async_trait]
impl SourceCatalogProvider for LegacyCatalogStub {
    async fn detail(
        &self,
        _source_id: &str,
        _endpoint: &str,
        _external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        Err(AppError::new(
            "LEGACY_CATALOG_PATH",
            ErrorKind::Unsupported,
            "通用目录路径不应被报刊来源使用",
            false,
        ))
    }
}

fn journal_record(
    issn_print: Option<&str>,
    issn_electronic: Option<&str>,
) -> PeriodicalJournalRecord {
    PeriodicalJournalRecord {
        title: "Nature Communications".to_owned(),
        issn_print: issn_print.and_then(Issn::parse),
        issn_electronic: issn_electronic.and_then(Issn::parse),
        publisher: Some("Nature Portfolio".to_owned()),
    }
}

fn article_record(
    pmcid: &str,
    title: &str,
    availability: PeriodicalArticleAvailability,
    journal: Option<PeriodicalJournalRecord>,
) -> PeriodicalArticleRecord {
    PeriodicalArticleRecord {
        source_key: EUROPE_PMC_SOURCE_KEY.to_owned(),
        remote_article_id: pmcid.to_owned(),
        journal,
        volume: Some(PeriodicalVolumeRecord {
            label: Some("15".to_owned()),
            number: Some(15.0),
            year: Some(2024),
        }),
        issue: Some(PeriodicalIssueRecord {
            label: Some("3-4".to_owned()),
            number: None,
            publication_date: Some("2024-03-15".to_owned()),
        }),
        title: title.to_owned(),
        doi: Doi::parse("10.1038/s41467-024-00001-2"),
        page_range: PageRange::new("1234", Some("1240")),
        ordinal: Some(12),
        availability,
        mime_type: Some("text/html; charset=utf-8".to_owned()),
    }
}

fn full_text_record(pmcid: &str, title: &str) -> PeriodicalArticleRecord {
    article_record(
        pmcid,
        title,
        PeriodicalArticleAvailability::FullText,
        Some(journal_record(Some("2041-1723"), None)),
    )
}

fn full_text_record_with_journal(
    pmcid: &str,
    title: &str,
    journal: PeriodicalJournalRecord,
) -> PeriodicalArticleRecord {
    article_record(
        pmcid,
        title,
        PeriodicalArticleAvailability::FullText,
        Some(journal),
    )
}

fn service(
    db: &Arc<Db>,
    provider: Arc<FakePeriodicalProvider>,
) -> (SourceImportService, Arc<SqliteRepositories>) {
    service_with_uow(db, provider, Arc::new(SqliteUnitOfWork::new(db.clone())))
}

fn service_with_uow(
    db: &Arc<Db>,
    provider: Arc<FakePeriodicalProvider>,
    uow: Arc<dyn UnitOfWork>,
) -> (SourceImportService, Arc<SqliteRepositories>) {
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let ports: Arc<dyn SourceImportPorts> = repos.clone();
    let repository: Arc<dyn PeriodicalRepository> = repos.clone();
    let registry_ports: Arc<dyn SourceRegistryPorts> = repos.clone();
    let service = SourceImportService::new(
        ports,
        uow,
        SourceRegistryService::new(registry_ports),
        Arc::new(LegacyCatalogStub),
    )
    .with_periodical_source(provider, repository);
    (service, repos)
}

/// 模拟竞态：事务已经提交，但调用方只收到一次冲突错误。真实并发下可能发生
/// 同样的观察窗口；服务必须重新读取文章来源身份并返回已提交的既有身份。
struct PersistThenReturnConflict {
    inner: SqliteUnitOfWork,
}

impl UnitOfWork for PersistThenReturnConflict {
    fn run_favorite(
        &self,
        _f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用收藏事务",
            false,
        ))
    }

    fn run_source_import(
        &self,
        _provider: &str,
        _external_id: &str,
        _work: &Work,
        _edition: &Edition,
        _items: &[MediaItem],
        _resources: &[Resource],
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用通用来源事务",
            false,
        ))
    }

    fn run_periodical_import(&self, plan: &PeriodicalImportPlan) -> Result<(), AppError> {
        self.inner.run_periodical_import(plan)?;
        Err(AppError::new(
            "PERIODICAL_ARTICLE_IMPORT_CONFLICT",
            ErrorKind::Conflict,
            "模拟已提交后的并发冲突",
            false,
        ))
    }
}

/// 真实数据库故障不能被期刊导入的并发重建吞掉或重复执行。
struct FailingPeriodicalDatabaseUow {
    calls: Arc<AtomicUsize>,
}

impl UnitOfWork for FailingPeriodicalDatabaseUow {
    fn run_favorite(
        &self,
        _f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用收藏事务",
            false,
        ))
    }

    fn run_source_import(
        &self,
        _provider: &str,
        _external_id: &str,
        _work: &Work,
        _edition: &Edition,
        _items: &[MediaItem],
        _resources: &[Resource],
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用通用来源事务",
            false,
        ))
    }

    fn run_periodical_import(&self, _plan: &PeriodicalImportPlan) -> Result<(), AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::new(
            "DATABASE_ERROR",
            ErrorKind::Database,
            "模拟不可重试的数据库故障",
            true,
        ))
    }
}

/// 篡改计划：Provider 观察被改成 `FullText`，但正文资源仍不可用。
///
/// 事务必须拒绝这种自相矛盾的计划，而不是把「没有正文」写成「来源有全文」。
struct FullTextObservationWithUnavailableResource {
    inner: SqliteUnitOfWork,
}

impl UnitOfWork for FullTextObservationWithUnavailableResource {
    fn run_favorite(
        &self,
        _f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用收藏事务",
            false,
        ))
    }

    fn run_source_import(
        &self,
        _provider: &str,
        _external_id: &str,
        _work: &Work,
        _edition: &Edition,
        _items: &[MediaItem],
        _resources: &[Resource],
    ) -> Result<(), AppError> {
        Err(AppError::new(
            "TEST_UOW_UNSUPPORTED",
            ErrorKind::Unsupported,
            "期刊集成测试不使用通用来源事务",
            false,
        ))
    }

    fn run_periodical_import(&self, plan: &PeriodicalImportPlan) -> Result<(), AppError> {
        let mut tampered = plan.clone();
        tampered.article.provider_content_availability = PeriodicalArticleAvailability::FullText;
        tampered.resource.availability = Availability::SourceUnavailable;
        self.inner.run_periodical_import(&tampered)
    }
}

fn table_count(db: &Db, table: &str) -> i64 {
    db.with_tx(|tx| {
        tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| {
            AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "读取测试表行数失败",
                false,
            )
            .with_source(error)
        })
    })
    .unwrap()
}

#[tokio::test]
async fn europepmc_import_builds_periodical_hierarchy_and_stays_idempotent() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![
        full_text_record(PMCID_A, "First article"),
        full_text_record(PMCID_B, "Second article"),
    ]));
    let (service, repos) = service(&db, provider.clone());

    let first: ImportedWork = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    let second = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_B)
        .await
        .unwrap();
    assert_eq!(
        first.work_id, second.work_id,
        "同一期刊的文章必须归属同一个期刊 Work"
    );
    assert_ne!(first.media_item_id, second.media_item_id);

    // 期刊层级：一个期刊、一个卷、一个期、两篇文章。
    let tree = service
        .periodical_tree(first.work_id)
        .await
        .unwrap()
        .expect("期刊层级必须存在");
    assert_eq!(tree.periodical.title, "Nature Communications");
    assert_eq!(
        tree.periodical.issn_print.as_ref().map(Issn::as_str),
        Some("2041-1723")
    );
    assert_eq!(tree.volumes.len(), 1, "同一卷身份必须复用");
    assert_eq!(tree.volumes[0].issues.len(), 1, "同一期身份必须复用");
    assert_eq!(tree.volumes[0].issues[0].articles.len(), 2);
    assert_eq!(
        tree.volumes[0].issues[0].issue.label.as_deref(),
        Some("3-4"),
        "不规则期号必须保留原文"
    );

    // 文章绑定既有 MediaItem，并且是 Article 媒介类型。
    let item = MediaItemRepository::get(&*repos, first.media_item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.media_type, MediaType::Article);
    assert_eq!(item.status, MediaItemStatus::Available);
    assert_eq!(item.title, "First article");
    assert_eq!(item.published_at.as_deref(), Some("2024-03-15"));
    let resources = ResourceRepository::list_by_media_item(&*repos, first.media_item_id)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].resource_type, ResourceType::ArticleSnapshot);
    assert_eq!(resources[0].availability, Availability::Available);

    // 期刊来源绑定身份由 ISSN 派生，而不是 PMCID。
    let refs = WorkRepository::list_source_refs(&*repos, first.work_id)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].provider, EUROPE_PMC_SOURCE_KEY);
    assert_eq!(refs[0].external_id, "europepmc:issn:2041-1723");

    // 重复导入同一 PMCID：命中既有归属，不再请求 Provider，也不新增层级。
    let calls_before = provider.calls();
    let repeat = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    assert_eq!(repeat, first);
    assert_eq!(
        provider.calls(),
        calls_before,
        "重复导入不得再次请求 Provider"
    );
    assert_eq!(table_count(&db, "periodical_articles"), 2);
    assert_eq!(table_count(&db, "periodical_volumes"), 1);
    assert_eq!(table_count(&db, "periodical_issues"), 1);

    let placement = service
        .periodical_placement(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap()
        .expect("文章归属链必须存在");
    assert_eq!(placement.work_id(), first.work_id);
    assert_eq!(placement.article.media_item_id, first.media_item_id);
    assert_eq!(placement.volume.number, Some(15.0));
    assert_eq!(
        placement.article.doi.as_ref().map(Doi::as_str),
        Some("10.1038/s41467-024-00001-2")
    );
    assert_eq!(
        placement
            .article
            .page_range
            .as_ref()
            .map(|range| (range.start.as_str(), range.end.as_deref())),
        Some(("1234", Some("1240")))
    );
}

#[tokio::test]
async fn metadata_only_articles_are_never_advertised_as_readable() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![article_record(
        PMCID_C,
        "Metadata only",
        PeriodicalArticleAvailability::MetadataOnly,
        Some(journal_record(Some("2041-1723"), None)),
    )]));
    let (service, repos) = service(&db, provider);

    let imported = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_C)
        .await
        .unwrap();
    let item = MediaItemRepository::get(&*repos, imported.media_item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        item.status,
        MediaItemStatus::Unavailable,
        "无正文条目不得标记为可读"
    );
    let resources = ResourceRepository::list_by_media_item(&*repos, imported.media_item_id)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].availability, Availability::SourceUnavailable);

    // Provider 的正文观察必须落到文章行上，否则 UI 只能看到笼统的「不可阅读」，
    // 无法把「来源只有元数据」与「这次没有观察到」分开。
    let placement = service
        .periodical_placement(EUROPE_PMC_SOURCE_KEY, PMCID_C)
        .await
        .unwrap()
        .expect("文章归属链必须存在");
    assert_eq!(
        placement.article.provider_content_availability,
        PeriodicalArticleAvailability::MetadataOnly
    );
}

/// 新导入按 Provider 本次观察写入正文可用性；FullText 与本次导入建立的正文资源
/// 事实一致，MetadataOnly 不建立可读资源。
#[tokio::test]
async fn imported_articles_persist_the_provider_content_observation() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![
        full_text_record(PMCID_A, "Full text article"),
        article_record(
            PMCID_C,
            "Metadata only article",
            PeriodicalArticleAvailability::MetadataOnly,
            Some(journal_record(Some("2041-1723"), None)),
        ),
    ]));
    let (service, repos) = service(&db, provider);

    let full_text = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    let metadata_only = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_C)
        .await
        .unwrap();
    assert_eq!(
        full_text.work_id, metadata_only.work_id,
        "同一期刊的两篇文章归属同一个期刊 Work"
    );

    for (pmcid, imported, observation, item_status, resource_availability) in [
        (
            PMCID_A,
            &full_text,
            PeriodicalArticleAvailability::FullText,
            MediaItemStatus::Available,
            Availability::Available,
        ),
        (
            PMCID_C,
            &metadata_only,
            PeriodicalArticleAvailability::MetadataOnly,
            MediaItemStatus::Unavailable,
            Availability::SourceUnavailable,
        ),
    ] {
        let placement = service
            .periodical_placement(EUROPE_PMC_SOURCE_KEY, pmcid)
            .await
            .unwrap()
            .expect("文章归属链必须存在");
        assert_eq!(
            placement.article.provider_content_availability, observation,
            "导入必须按 Provider 观察写入正文可用性"
        );
        assert_eq!(placement.article.media_item_id, imported.media_item_id);

        let item = MediaItemRepository::get(&*repos, imported.media_item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, item_status);
        let resources = ResourceRepository::list_by_media_item(&*repos, imported.media_item_id)
            .await
            .unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(
            resources[0].availability, resource_availability,
            "可读资源事实必须与 Provider 观察一致"
        );
        if observation == PeriodicalArticleAvailability::MetadataOnly {
            assert_eq!(resources[0].mime_type, None, "元数据条目不声明正文 MIME");
        }
    }

    // 层级读取路径同样带上正文观察，UI 才有可辨识的事实可用。
    let tree = service
        .periodical_tree(full_text.work_id)
        .await
        .unwrap()
        .expect("期刊层级必须存在");
    let mut observed: Vec<String> = tree.volumes[0].issues[0]
        .articles
        .iter()
        .map(|article| format!("{:?}", article.provider_content_availability))
        .collect();
    observed.sort();
    assert_eq!(observed, vec!["FullText", "MetadataOnly"]);
}

#[tokio::test]
async fn failed_periodical_write_leaves_no_source_ref_or_orphan_hierarchy() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![full_text_record(
        PMCID_A,
        "Rollback article",
    )]));
    let (service, _repos) = service(&db, provider);

    // 确定性失败：文章行写入被 trigger 拒绝。整次导入必须回滚。
    db.with_tx(|tx| {
        tx.execute_batch(
            "CREATE TRIGGER fail_periodical_article_insert
             BEFORE INSERT ON periodical_articles
             BEGIN
                 SELECT RAISE(ABORT, 'injected periodical article failure');
             END;",
        )
        .map_err(|error| {
            AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "创建测试触发器失败",
                false,
            )
            .with_source(error)
        })?;
        Ok(())
    })
    .unwrap();

    let error = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");

    for table in [
        "works",
        "work_source_refs",
        "editions",
        "media_items",
        "resources",
        "periodicals",
        "periodical_volumes",
        "periodical_issues",
        "periodical_articles",
    ] {
        assert_eq!(
            table_count(&db, table),
            0,
            "失败的报刊导入不得在 {table} 留下任何行"
        );
    }
}

#[tokio::test]
async fn article_without_issn_falls_back_to_the_legacy_import_path() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![article_record(
        PMCID_A,
        "No ISSN article",
        PeriodicalArticleAvailability::FullText,
        Some(journal_record(None, None)),
    )]));
    let (service, _repos) = service(&db, provider);

    let error = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap_err();
    assert_eq!(
        error.code().as_str(),
        "LEGACY_CATALOG_PATH",
        "没有 ISSN 时不得建立期刊层级，必须回退到既有单篇导入"
    );
    for table in [
        "works",
        "periodicals",
        "periodical_volumes",
        "periodical_issues",
        "periodical_articles",
    ] {
        assert_eq!(table_count(&db, table), 0, "{table} 不得有猜测出来的层级");
    }
}

#[tokio::test]
async fn periodical_repository_reads_articles_by_source_identity() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![full_text_record(
        PMCID_A,
        "First article",
    )]));
    let (service, repos) = service(&db, provider);
    let imported = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();

    let periodical = PeriodicalRepository::find_by_work(&*repos, imported.work_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        PeriodicalRepository::find_by_issn(&*repos, &Issn::parse("2041-1723").unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        periodical.id
    );
    let volumes = PeriodicalRepository::list_volumes(&*repos, periodical.id)
        .await
        .unwrap();
    assert_eq!(volumes.len(), 1);
    let issues = PeriodicalRepository::list_issues(&*repos, volumes[0].id)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    let articles = PeriodicalRepository::list_articles(&*repos, issues[0].id)
        .await
        .unwrap();
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0].media_item_id, imported.media_item_id);
    assert!(
        PeriodicalRepository::find_article_by_source(&*repos, EUROPE_PMC_SOURCE_KEY, "PMC9999999")
            .await
            .unwrap()
            .is_none()
    );
    let _: MediaItemId = articles[0].media_item_id;
}

#[tokio::test]
async fn print_only_then_dual_issn_enriches_the_same_periodical_work() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![
        full_text_record(PMCID_A, "Print-only article"),
        full_text_record_with_journal(
            PMCID_B,
            "Dual-identity article",
            journal_record(Some("2041-1723"), Some("1420-682X")),
        ),
    ]));
    let (service, repos) = service(&db, provider);

    let first = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    let second = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_B)
        .await
        .unwrap();

    assert_eq!(first.work_id, second.work_id);
    let periodical = PeriodicalRepository::find_by_work(&*repos, first.work_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        periodical.issn_print.as_ref().map(Issn::as_str),
        Some("2041-1723")
    );
    assert_eq!(
        periodical.issn_electronic.as_ref().map(Issn::as_str),
        Some("1420-682X")
    );
    assert_eq!(table_count(&db, "periodicals"), 1);

    let refs = WorkRepository::list_source_refs(&*repos, first.work_id)
        .await
        .unwrap();
    let external_ids: Vec<&str> = refs
        .iter()
        .map(|reference| reference.external_id.as_str())
        .collect();
    assert!(external_ids.contains(&"europepmc:issn:2041-1723"));
    assert!(external_ids.contains(&"europepmc:issn:1420-682X"));
}

#[tokio::test]
async fn first_dual_issn_import_persists_all_periodical_work_source_refs() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![
        full_text_record_with_journal(
            PMCID_A,
            "Dual identity article",
            journal_record(Some("2041-1723"), Some("1420-682X")),
        ),
    ]));
    let (service, repos) = service(&db, provider);

    let imported = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    let refs = WorkRepository::list_source_refs(&*repos, imported.work_id)
        .await
        .unwrap();
    let external_ids: Vec<&str> = refs
        .iter()
        .map(|reference| reference.external_id.as_str())
        .collect();

    assert_eq!(refs.len(), 2);
    assert!(external_ids.contains(&"europepmc:issn:2041-1723"));
    assert!(external_ids.contains(&"europepmc:issn:1420-682X"));
}

#[tokio::test]
async fn later_richer_observations_enrich_the_existing_volume_and_issue() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let mut incomplete = full_text_record(PMCID_A, "Incomplete attribution");
    incomplete.volume = Some(PeriodicalVolumeRecord {
        label: Some("15".to_owned()),
        number: None,
        year: None,
    });
    incomplete.issue = Some(PeriodicalIssueRecord {
        label: Some("1".to_owned()),
        number: None,
        publication_date: None,
    });

    let mut richer = full_text_record(PMCID_B, "Richer attribution");
    richer.volume = Some(PeriodicalVolumeRecord {
        label: Some("15".to_owned()),
        number: Some(15.0),
        year: Some(2024),
    });
    richer.issue = Some(PeriodicalIssueRecord {
        label: Some("1".to_owned()),
        number: Some(1.0),
        publication_date: Some("2024-03-15".to_owned()),
    });

    let provider = Arc::new(FakePeriodicalProvider::new(vec![incomplete, richer]));
    let (service, _repos) = service(&db, provider);
    let first = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_B)
        .await
        .unwrap();

    let tree = service
        .periodical_tree(first.work_id)
        .await
        .unwrap()
        .expect("期刊层级必须存在");
    assert_eq!(tree.volumes.len(), 1, "标签与编号变化不能制造重复卷");
    assert_eq!(tree.volumes[0].volume.number, Some(15.0));
    assert_eq!(tree.volumes[0].volume.year, Some(2024));
    assert_eq!(
        tree.volumes[0].issues.len(),
        1,
        "标签与编号变化不能制造重复期"
    );
    let issue = &tree.volumes[0].issues[0].issue;
    assert_eq!(issue.number, Some(1.0));
    assert_eq!(issue.publication_date.as_deref(), Some("2024-03-15"));
}

#[tokio::test]
async fn dual_issn_observation_rejects_split_periodical_identities() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![
        full_text_record_with_journal(
            PMCID_A,
            "Print-only article",
            journal_record(Some("2041-1723"), None),
        ),
        full_text_record_with_journal(
            PMCID_B,
            "Electronic-only article",
            journal_record(None, Some("1420-682X")),
        ),
        full_text_record_with_journal(
            PMCID_C,
            "Ambiguous dual article",
            journal_record(Some("2041-1723"), Some("1420-682X")),
        ),
    ]));
    let (service, repos) = service(&db, provider);

    let print_only = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .unwrap();
    let electronic_only = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_B)
        .await
        .unwrap();
    assert_ne!(print_only.work_id, electronic_only.work_id);

    let error = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_C)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "PERIODICAL_IDENTITY_CONFLICT");
    assert_eq!(table_count(&db, "periodical_articles"), 2);
    assert_eq!(table_count(&db, "periodicals"), 2);
    assert!(
        PeriodicalRepository::find_article_by_source(&*repos, EUROPE_PMC_SOURCE_KEY, PMCID_C)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn committed_concurrent_periodical_import_is_recovered_by_source_identity() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![full_text_record(
        PMCID_A,
        "Recovered article",
    )]));
    let uow = Arc::new(PersistThenReturnConflict {
        inner: SqliteUnitOfWork::new(db.clone()),
    });
    let (service, repos) = service_with_uow(&db, provider.clone(), uow);

    let imported = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .expect("已提交后收到冲突仍应恢复既有身份");

    assert_eq!(table_count(&db, "periodical_articles"), 1);
    let placement =
        PeriodicalRepository::find_article_by_source(&*repos, EUROPE_PMC_SOURCE_KEY, PMCID_A)
            .await
            .unwrap()
            .expect("恢复必须读取刚提交的来源文章");
    assert_eq!(placement.work_id(), imported.work_id);
    assert_eq!(placement.article.media_item_id, imported.media_item_id);
    assert_eq!(provider.calls(), 1);
}

#[tokio::test]
async fn database_failures_are_not_retried_as_periodical_conflicts() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![full_text_record(
        PMCID_A,
        "Database failure",
    )]));
    let calls = Arc::new(AtomicUsize::new(0));
    let uow = Arc::new(FailingPeriodicalDatabaseUow {
        calls: calls.clone(),
    });
    let (service, _repos) = service_with_uow(&db, provider, uow);

    let error = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .expect_err("真实数据库故障必须直接返回");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "数据库故障不得被重试");
    for table in [
        "works",
        "work_source_refs",
        "editions",
        "media_items",
        "resources",
        "periodicals",
        "periodical_volumes",
        "periodical_issues",
        "periodical_articles",
    ] {
        assert_eq!(
            table_count(&db, table),
            0,
            "失败的导入不得在 {table} 留下行"
        );
    }
}

/// Provider 的正文观察与本次导入建立的正文资源事实必须一致：
/// `FullText` 不能配上不可用的正文资源，否则页面会把不存在的正文写成来源有全文。
#[tokio::test]
async fn full_text_observation_requires_a_readable_content_resource() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let provider = Arc::new(FakePeriodicalProvider::new(vec![full_text_record(
        PMCID_A,
        "Contradictory article",
    )]));
    let uow = Arc::new(FullTextObservationWithUnavailableResource {
        inner: SqliteUnitOfWork::new(db.clone()),
    });
    let (service, _repos) = service_with_uow(&db, provider, uow);

    let error = service
        .import_content_candidate(EUROPE_PMC_SOURCE_KEY, PMCID_A)
        .await
        .expect_err("自相矛盾的计划必须被拒绝");
    assert_eq!(error.code().as_str(), "PERIODICAL_IMPORT_PLAN_INVALID");
    for table in [
        "works",
        "editions",
        "media_items",
        "resources",
        "periodicals",
        "periodical_volumes",
        "periodical_issues",
        "periodical_articles",
    ] {
        assert_eq!(
            table_count(&db, table),
            0,
            "被拒绝的计划不得在 {table} 留下行"
        );
    }
}

// ---------- 用户登记的 RSS/Atom 订阅源（Task 1） ----------

/// 订阅源抓取替身：进程内返回固定文档并记录被请求的地址，因此可以在没有网络的
/// 情况下证明「只请求登记端点、绝不请求条目 link」。
struct FakeFeedSource {
    body: String,
    requested: Mutex<Vec<String>>,
}

impl FakeFeedSource {
    fn new(body: &str) -> Arc<Self> {
        Arc::new(Self {
            body: body.to_owned(),
            requested: Mutex::new(Vec::new()),
        })
    }

    fn requested(&self) -> Vec<String> {
        self.requested
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[async_trait]
impl FeedSource for FakeFeedSource {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        self.requested
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(url.to_owned());
        Ok(self.body.clone())
    }
}

const FEED_ENDPOINT: &str = "https://feeds.example.invalid/rss.xml";
const FEED_ENTRY_ONE: &str = "post-1";

const FEED_RSS_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>我的订阅</title>
    <link>https://feeds.example.invalid/</link>
    <item>
      <title>订阅第一篇</title>
      <link>https://posts.example.invalid/one</link>
      <guid isPermaLink="false">post-1</guid>
      <pubDate>Wed, 01 Oct 2026 08:00:00 GMT</pubDate>
      <description>&lt;p&gt;摘要第一段&lt;/p&gt;</description>
      <content:encoded>&lt;p&gt;正文第一段&lt;/p&gt;&lt;img src="https://cdn.example.invalid/a.png"/&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;p&gt;正文第二段&lt;/p&gt;</content:encoded>
    </item>
    <item>
      <title>订阅第二篇</title>
      <link>https://posts.example.invalid/two</link>
      <guid isPermaLink="false">post-2</guid>
      <pubDate>Thu, 02 Oct 2026 08:00:00 GMT</pubDate>
      <description>只有摘要</description>
    </item>
  </channel>
</rss>"#;

fn feed_registry() -> (Arc<Db>, Arc<SqliteRepositories>, SourceRegistryService) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let registry = SourceRegistryService::new(repos.clone());
    (db, repos, registry)
}

/// 订阅源导入服务：与真实组合根一致，Repository、UnitOfWork、注册表与目录
/// Provider 共享同一个内存数据库句柄。
fn feed_import_service(
    db: &Arc<Db>,
    repos: &Arc<SqliteRepositories>,
    registry: &SourceRegistryService,
    provider: Arc<FeedProvider>,
) -> SourceImportService {
    let import_ports: Arc<dyn SourceImportPorts> = repos.clone();
    SourceImportService::new(
        import_ports,
        Arc::new(SqliteUnitOfWork::new(db.clone())),
        registry.clone(),
        Arc::new(LegacyCatalogStub),
    )
    .with_feed_source(provider)
}

#[tokio::test]
async fn feed_source_search_import_read_and_snapshot_stay_on_the_registered_feed() {
    let (db, repos, registry) = feed_registry();
    let source_id = registry
        .add_feed_source("我的订阅", FEED_ENDPOINT)
        .await
        .unwrap()
        .source_id;
    let source = FakeFeedSource::new(FEED_RSS_FIXTURE);
    let provider = Arc::new(FeedProvider::new(registry.clone(), source.clone()));
    let participant = FeedSearchParticipant::new(registry.clone(), source.clone());
    let service = feed_import_service(&db, &repos, &registry, provider.clone());

    // 1) 搜索：只给出 opaque 候选，不携带端点、条目 link 或正文。
    let not_cancelled = || false;
    let cards = participant
        .search_for(&source_id, "第一篇", 10, &not_cancelled)
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title, "订阅第一篇");
    let handle = cards[0].work_id.clone();
    assert!(handle.starts_with("content-candidate-"));
    assert!(handle.contains(&source_id), "候选句柄只携带来源身份");
    assert!(!handle.contains("feeds.example.invalid"));
    assert!(!handle.contains("posts.example.invalid"));
    assert!(!handle.contains("正文第一段"));

    // 2) 导入：建立真实 Article 与受控 SourceObject，不写正文文件。
    let temp = tempfile::tempdir().unwrap();
    let imported = service.import_candidate(&handle).await.unwrap();

    let editions = repos.list_by_work(imported.work_id).await.unwrap();
    assert_eq!(editions.len(), 1);
    assert_eq!(editions[0].edition_type, MediaType::Article);
    let items = repos.list_by_edition(editions[0].id).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].media_type, MediaType::Article);
    let resources = repos
        .list_by_media_item(imported.media_item_id)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].resource_type, ResourceType::ArticleSnapshot);
    assert!(
        resources[0].storage_location_id.is_none(),
        "导入阶段不得登记本地存储位置"
    );
    assert!(
        std::fs::read_dir(temp.path()).unwrap().next().is_none(),
        "导入阶段不得写正文文件"
    );
    let haven_domain::entities::ResourceLocator::SourceObject { remote_id, .. } =
        &resources[0].locator
    else {
        panic!("订阅导入必须写入 SourceObject");
    };
    let remote_id = remote_id.clone();

    // 3) 在线正文：只消费订阅源自带且清洗后的内容。
    let body = RemoteSessionPort::read(provider.as_ref(), FEED_SOURCE_KEY, &remote_id, None)
        .await
        .unwrap();
    assert_eq!(body.mime_type, "text/html; charset=utf-8");
    let html = String::from_utf8(body.bytes).unwrap();
    assert!(html.contains("正文第一段"));
    assert!(html.contains("正文第二段"));
    assert!(!html.contains("<script"), "脚本必须被剥离");
    assert!(!html.contains("alert(1)"));
    assert!(!html.contains("<img"));
    assert!(!html.contains("cdn.example.invalid"), "外链资源必须被剥离");
    assert!(
        !html.contains("posts.example.invalid"),
        "不得请求或消费条目链接"
    );

    // 4) 离线快照：显式获取才写盘，内容与在线会话一致。
    let snapshot = temp.path().join("snapshot.html");
    let acquired =
        RemoteAcquisitionPort::acquire(provider.as_ref(), FEED_SOURCE_KEY, &remote_id, &snapshot)
            .await
            .unwrap();
    assert_eq!(acquired.mime, "text/html; charset=utf-8");
    assert_eq!(
        std::fs::read_to_string(&snapshot).unwrap(),
        html,
        "在线与离线必须消费同一份清洗后的正文"
    );

    // 全程只请求过登记端点；条目 link 从未被请求。
    let requested = source.requested();
    assert!(!requested.is_empty());
    assert!(
        requested.iter().all(|url| url == FEED_ENDPOINT),
        "订阅源 Provider 只允许请求登记端点，实际请求: {requested:?}"
    );
}

#[tokio::test]
async fn repeated_feed_import_is_idempotent_and_malformed_feeds_fail_closed() {
    let (db, repos, registry) = feed_registry();
    let source_id = registry
        .add_feed_source("我的订阅", FEED_ENDPOINT)
        .await
        .unwrap()
        .source_id;
    let source = FakeFeedSource::new(FEED_RSS_FIXTURE);
    let provider = Arc::new(FeedProvider::new(registry.clone(), source.clone()));
    let service = feed_import_service(&db, &repos, &registry, provider.clone());

    let first = service
        .import_feed_candidate(&source_id, &feed_entry_digest(FEED_ENTRY_ONE))
        .await
        .unwrap();
    let second = service
        .import_feed_candidate(&source_id, &feed_entry_digest(FEED_ENTRY_ONE))
        .await
        .unwrap();
    assert_eq!(first, second, "重复导入必须命中同一作品身份");
    assert_eq!(table_count(&db, "works"), 1);
    assert_eq!(table_count(&db, "media_items"), 1);
    assert_eq!(table_count(&db, "resources"), 1);

    // 畸形文档必须明确失败，而不是返回空结果或半截条目。
    let broken =
        FakeFeedSource::new("<rss><channel><item><title>标签不匹配</wrong></channel></rss>");
    let broken_provider = FeedProvider::new(registry.clone(), broken);
    let error = broken_provider
        .detail(&source_id, "", &feed_entry_digest(FEED_ENTRY_ONE))
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    assert!(
        !error.user_message().contains("feeds.example.invalid"),
        "错误文案不得回显订阅源端点"
    );

    // 已删除或未登记的订阅源身份：这是「来源当前不可用」，不是调用方参数错误，
    // 必须是稳定的可重试来源错误，且不发起任何请求。
    let unknown = FakeFeedSource::new(FEED_RSS_FIXTURE);
    let unknown_provider = FeedProvider::new(registry.clone(), unknown.clone());
    let error = unknown_provider
        .detail(
            "custom_feed_ffffffffffff",
            "",
            &feed_entry_digest(FEED_ENTRY_ONE),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    assert!(error.retryable());
    assert!(
        unknown.requested().is_empty(),
        "没有登记端点时不得发起任何请求"
    );
}

// ---------- 订阅源搜索分发（Task 1） ----------

struct FeedCaptureSink {
    events: Mutex<Vec<haven_application::wire::SearchSourceEvent>>,
}

impl FeedCaptureSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    fn snapshot(&self) -> Vec<haven_application::wire::SearchSourceEvent> {
        self.events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl SearchEventSink for FeedCaptureSink {
    fn emit_search_event(&self, event: haven_application::wire::SearchSourceEvent) {
        self.events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    }
}

/// 分发级回归：`custom_feed_*` 必须由最长前缀路由交给订阅源参与者，而不是落到
/// 自定义 OPDS 参与者上。这里注册的是组合根同款前缀（`custom_` vs
/// `custom_feed_`），因此断言的是真实的解析/调度语义，而不是参与者自身的搜索。
#[tokio::test]
async fn feed_sources_dispatch_to_the_feed_participant_not_the_custom_opds_participant() {
    use haven_application::services::search_source::SearchSourceService;
    use haven_application::wire::{SearchSourceEventKind, SearchSourceStartRequest};
    use haven_infrastructure::opds::{CUSTOM_OPDS_ID_PREFIX, OpdsClient, OpdsSearchParticipant};

    let (_db, _repos, registry) = feed_registry();
    let source_id = registry
        .add_feed_source("我的订阅", FEED_ENDPOINT)
        .await
        .unwrap()
        .source_id;
    registry
        .set_custom_source_enabled(&source_id, true)
        .await
        .unwrap();

    let source = FakeFeedSource::new(FEED_RSS_FIXTURE);
    let opds_client = Arc::new(OpdsClient::new().unwrap());
    let participants: Vec<Arc<dyn SearchSourceParticipant>> = vec![
        Arc::new(OpdsSearchParticipant::new(
            CUSTOM_OPDS_ID_PREFIX.to_owned(),
            registry.clone(),
            opds_client,
        )),
        Arc::new(FeedSearchParticipant::new(registry.clone(), source.clone())),
    ];
    let sink = FeedCaptureSink::new();
    let service = SearchSourceService::new(registry.clone(), participants, sink.clone());

    service
        .start(SearchSourceStartRequest {
            query: "订阅".to_owned(),
            category: None,
            limit_per_source: Some(10),
        })
        .await
        .unwrap();

    for _ in 0..200 {
        let terminal = sink.snapshot().iter().any(|event| {
            matches!(
                event.kind,
                SearchSourceEventKind::Completed
                    | SearchSourceEventKind::Failed
                    | SearchSourceEventKind::Cancelled
            )
        });
        if terminal {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let events = sink.snapshot();
    let result = events
        .iter()
        .find(|event| event.kind == SearchSourceEventKind::SourceResult)
        .expect("订阅源参与者必须返回结果");
    assert_eq!(result.data.source_id.as_deref(), Some(source_id.as_str()));
    assert_eq!(result.data.works.len(), 2);
    for card in &result.data.works {
        assert!(card.work_id.starts_with("content-candidate-"));
        assert!(!card.work_id.contains("feeds.example.invalid"));
        assert!(!card.work_id.contains("posts.example.invalid"));
    }
    assert!(
        !events
            .iter()
            .any(|event| event.kind == SearchSourceEventKind::Warning),
        "订阅源不得被自定义 OPDS 参与者接管而告警"
    );
    assert_eq!(
        source.requested(),
        vec![FEED_ENDPOINT.to_owned()],
        "只有订阅源参与者允许请求登记端点"
    );
}
