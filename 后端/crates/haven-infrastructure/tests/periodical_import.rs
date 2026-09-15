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
    FavoriteTxPorts, PeriodicalImportPlan, SourceImportPorts, SourceRegistryPorts, UnitOfWork,
};
use haven_application::services::source_import::{
    ImportedWork, SourceCatalogEntry, SourceCatalogProvider, SourceImportService,
};
use haven_application::services::source_registry::SourceRegistryService;
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    MediaItemRepository, PeriodicalRepository, ResourceRepository, WorkRepository,
};
use haven_domain::entities::{Edition, MediaItem, Resource, Work};
use haven_domain::enums::{Availability, MediaItemStatus, MediaType, ResourceType};
use haven_domain::ids::MediaItemId;
use haven_domain::periodical::{Doi, Issn, PageRange};
use haven_infrastructure::Db;
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
