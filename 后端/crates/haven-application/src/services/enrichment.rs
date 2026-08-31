//! EnrichmentService：元数据自动流水线（契约 §36.8，V2-F 批次）。
//!
//! 流程：扫描完成后对新 Work 入队 → CMS10 标题精确匹配 → 命中走既有
//! `SourceImportService`（external_ids 去重 work_upsert，幂等）→ enriched；
//! 未命中/失败标 failed，不回滚扫描（保留原始名 Work）。
//! 状态经 `enrichment_status` 查询；变更以 `metadata.changed` 事件广播。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{EnrichmentRepository, EnrichmentState, WorkRepository};
use haven_domain::ids::WorkId;

use crate::services::ports::EnrichmentPorts;
use crate::services::source_import::SourceImportService;
use crate::wire::{EnrichmentStateDto, EnrichmentStatusWire};

/// `metadata.changed` 事件出口（application 构造负载，transport 层负责投递）。
pub trait MetadataChangedSink: Send + Sync {
    fn emit_metadata_changed(&self, event: crate::wire::MetadataChangedDto);
}

/// 单个 Work 的 enrichment 结果（供扫描钩子聚合日志/测试断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichedWorkOutcome {
    pub work_id: WorkId,
    pub status: EnrichmentStatusWire,
}

const CMS10_SOURCE_ID: &str = "cms10";
/// 单次流水线最大处理数（防扫描后风暴；剩余留待下次触发）。
const MAX_BATCH: usize = 50;

#[derive(Clone)]
pub struct EnrichmentService {
    ports: Arc<dyn EnrichmentPorts>,
    import: SourceImportService,
}

impl EnrichmentService {
    pub fn new(ports: Arc<dyn EnrichmentPorts>, import: SourceImportService) -> Self {
        Self { ports, import }
    }

    /// `enrichment_status`：workId=null 返回全部记录（updated_at 倒序）。
    pub async fn status(
        &self,
        work_id: Option<String>,
    ) -> Result<Vec<EnrichmentStateDto>, AppError> {
        let filter = match work_id.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => Some(parse_work_id(raw)?),
        };
        let states = EnrichmentRepository::list(&*self.ports, filter).await?;
        Ok(states
            .into_iter()
            .map(|s| EnrichmentStateDto {
                work_id: s.work_id.to_string(),
                status: parse_status(&s.status),
                source_id: s.source_id,
                error: s.error,
            })
            .collect())
    }

    /// 扫描完成钩子：对尚无 enrichment 记录的 Work 逐个入队执行。
    /// 单个 Work 失败不影响批次其余项；返回逐条结果。
    pub async fn run_pending(&self) -> Result<Vec<EnrichedWorkOutcome>, AppError> {
        // 无 external 引用且无记录的 Work 即"新作品"。
        let candidates: Vec<WorkId> = {
            let all = WorkRepository::list(&*self.ports, MAX_BATCH as u32, 0).await?;
            let mut pending = Vec::new();
            for work in all {
                if EnrichmentRepository::get(&*self.ports, work.id)
                    .await?
                    .is_none()
                    && !WorkRepository::has_any_source_ref(&*self.ports, work.id).await?
                {
                    pending.push(work.id);
                    if pending.len() >= MAX_BATCH {
                        break;
                    }
                }
            }
            pending
        };

        let mut outcomes = Vec::new();
        for work_id in candidates {
            outcomes.push(self.enrich_one(work_id).await?);
        }
        Ok(outcomes)
    }

    /// 对单个 Work 执行一次 enrichment（状态机 pending → enriched|failed）。
    async fn enrich_one(&self, work_id: WorkId) -> Result<EnrichedWorkOutcome, AppError> {
        self.record(work_id, "pending", None, None).await?;

        let work = WorkRepository::get(&*self.ports, work_id)
            .await?
            .ok_or_else(|| {
                AppError::new(
                    "RESOURCE_NOT_FOUND",
                    ErrorKind::NotFound,
                    "作品不存在",
                    false,
                )
            })?;

        let outcome = match self
            .import
            .match_cms10_by_title(&work.canonical_title)
            .await
        {
            Ok(Some(_import)) => EnrichedWorkOutcome {
                work_id,
                status: EnrichmentStatusWire::Enriched,
            },
            Ok(None) => EnrichedWorkOutcome {
                work_id,
                status: EnrichmentStatusWire::Failed,
            },
            Err(err) => {
                self.record(work_id, "failed", None, Some(&safe_error(&err)))
                    .await?;
                return Ok(EnrichedWorkOutcome {
                    work_id,
                    status: EnrichmentStatusWire::Failed,
                });
            }
        };

        match outcome.status {
            EnrichmentStatusWire::Enriched => {
                self.record(work_id, "enriched", Some(CMS10_SOURCE_ID), None)
                    .await?;
            }
            _ => {
                self.record(work_id, "failed", None, Some("未在来源中找到匹配条目"))
                    .await?;
            }
        }
        Ok(outcome)
    }

    async fn record(
        &self,
        work_id: WorkId,
        status: &str,
        source_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        let state = EnrichmentState {
            work_id,
            status: status.to_string(),
            source_id: source_id.map(str::to_string),
            error: error.map(str::to_string),
            updated_at: UtcMillis::now(),
        };
        EnrichmentRepository::upsert(&*self.ports, &state).await
    }
}

fn parse_work_id(raw: &str) -> Result<WorkId, AppError> {
    raw.parse::<WorkId>().map_err(|_| {
        AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "workId 形状非法",
            false,
        )
    })
}

fn parse_status(raw: &str) -> EnrichmentStatusWire {
    match raw {
        "enriched" => EnrichmentStatusWire::Enriched,
        "failed" => EnrichmentStatusWire::Failed,
        _ => EnrichmentStatusWire::Pending,
    }
}

/// 只保留错误分类文案与稳定 Code（不含内部路径/远端响应）。
fn safe_error(err: &AppError) -> String {
    format!("{}: {}", err.code().as_str(), err.user_message())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ports::EnrichmentPorts;
    use haven_domain::contracts::WorkRepository;
    use haven_domain::entities::{ArtworkSet, Work};
    use haven_domain::enums::{WorkStatus, WorkType};
    use std::sync::Arc;

    /// 测试目录：只按标题精确命中"三体"，其余返回空。
    struct FakeCatalog;

    #[async_trait::async_trait]
    impl crate::services::source_import::SourceCatalogProvider for FakeCatalog {
        async fn detail(
            &self,
            _source_id: &str,
            _endpoint: &str,
            external_id: &str,
        ) -> Result<crate::services::SourceCatalogEntry, AppError> {
            Ok(crate::services::SourceCatalogEntry {
                external_id: external_id.to_string(),
                title: "三体".into(),
                year: Some(2023),
                type_name: Some("科幻".into()),
                pic: Some("https://img.example.com/p.jpg".into()),
                episodes: vec![("01".into(), "http://example.com/1.m3u8".into())],
                content: Some("科幻小说".into()),
                director: Some("刘慈欣".into()),
                actor: Some("演员甲".into()),
                local_file: None,
                media_type: None,
                remote: None,
            })
        }

        async fn search(
            &self,
            _source_id: &str,
            _endpoint: &str,
            query: &str,
            _limit: u32,
        ) -> Result<Vec<crate::services::SourceCatalogEntry>, AppError> {
            if query == "三体" {
                Ok(vec![crate::services::SourceCatalogEntry {
                    external_id: "v-001".into(),
                    title: "三体".into(),
                    year: Some(2023),
                    type_name: Some("科幻".into()),
                    pic: Some("https://img.example.com/p.jpg".into()),
                    episodes: vec![("01".into(), "http://example.com/1.m3u8".into())],
                    content: Some("科幻小说".into()),
                    director: Some("刘慈欣".into()),
                    actor: Some("演员甲".into()),
                    local_file: None,
                    media_type: None,
                    remote: None,
                }])
            } else {
                Ok(Vec::new())
            }
        }
    }

    async fn fixture() -> (
        EnrichmentService,
        Arc<dyn EnrichmentPorts>,
        crate::services::source_registry::SourceRegistryService,
    ) {
        let db = Arc::new(haven_infrastructure::db::Db::open_in_memory().unwrap());
        let repos = Arc::new(haven_infrastructure::db::repos::SqliteRepositories::new(db));
        let import_ports: Arc<dyn crate::services::ports::SourceImportPorts> = repos.clone();
        let registry_ports: Arc<dyn crate::services::ports::SourceRegistryPorts> = repos.clone();
        let enrich_ports: Arc<dyn EnrichmentPorts> = repos.clone();
        let registry =
            crate::services::source_registry::SourceRegistryService::new(registry_ports.clone());
        let import =
            SourceImportService::new(import_ports, registry.clone(), Arc::new(FakeCatalog));
        // 预配置端点（settings KV 直写）。
        registry
            .set_endpoint(CMS10_SOURCE_ID, "http://ep.example.com")
            .await
            .unwrap();
        (
            EnrichmentService::new(enrich_ports, import),
            repos,
            registry,
        )
    }

    async fn seed_work(ports: &dyn EnrichmentPorts, title: &str) -> WorkId {
        let work = Work {
            id: WorkId::new(),
            canonical_title: title.into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: ArtworkSet::default(),
            created_at: UtcMillis::now(),
            updated_at: UtcMillis::now(),
        };
        WorkRepository::save(ports, &work).await.unwrap();
        work.id
    }

    #[tokio::test]
    async fn run_pending_enriches_exact_title_match_and_fails_miss() {
        let (svc, ports, _registry) = fixture().await;
        let hit = seed_work(ports.as_ref(), "三体").await;
        let miss = seed_work(ports.as_ref(), "不存在的作品").await;

        let outcomes = svc.run_pending().await.unwrap();
        assert_eq!(outcomes.len(), 2);

        let hit_state = svc.status(Some(hit.to_string())).await.unwrap();
        assert_eq!(hit_state.len(), 1);
        assert_eq!(hit_state[0].status, EnrichmentStatusWire::Enriched);
        assert_eq!(hit_state[0].source_id.as_deref(), Some("cms10"));

        let miss_state = svc.status(Some(miss.to_string())).await.unwrap();
        assert_eq!(miss_state[0].status, EnrichmentStatusWire::Failed);
        assert!(miss_state[0].error.is_some(), "failed 必须携带安全文案");
    }

    #[tokio::test]
    async fn run_pending_skips_already_processed_works() {
        let (svc, ports, _registry) = fixture().await;
        let _id = seed_work(ports.as_ref(), "三体").await;
        svc.run_pending().await.unwrap();

        // 第二次运行：已有记录的 Work 不再入队。
        let second = svc.run_pending().await.unwrap();
        assert!(second.is_empty(), "已处理 Work 不得重复入队");

        // 已有来源引用的 Work 也跳过（模拟用户手动导入过）。
        let fresh = seed_work(ports.as_ref(), "手动导入").await;
        haven_domain::contracts::WorkRepository::save_source_ref(
            ports.as_ref(),
            "manual",
            "ext-1",
            fresh,
        )
        .await
        .unwrap();
        assert!(svc.run_pending().await.unwrap().is_empty());
        assert_eq!(
            svc.status(Some(fresh.to_string())).await.unwrap().len(),
            0,
            "跳过的 Work 无 enrichment 记录"
        );
    }

    #[tokio::test]
    async fn status_rejects_malformed_work_id() {
        let (svc, _ports, _registry) = fixture().await;
        let err = svc.status(Some("not-a-uuid".into())).await;
        assert_eq!(err.unwrap_err().code().as_str(), "INVALID_ARGUMENT");
        assert!(svc.status(None).await.unwrap().is_empty());
    }
}
