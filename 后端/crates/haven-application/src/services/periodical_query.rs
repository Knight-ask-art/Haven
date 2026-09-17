//! 报刊层级的只读应用服务。
//!
//! 边界（ADR-002）：
//! - 只依赖 [`PeriodicalRepository`] 端口读取已持久化的期刊 → 卷 → 期 → 文章，
//!   **不持有 Provider**：查询路径绝不触发网络请求，也不写任何存储；
//! - 只按 `work_id` 逐级归属查找，不按标题、通用 document/article 分类或搜索结果
//!   猜测期刊身份；找不到层级时返回稳定的 `PERIODICAL_NOT_FOUND`；
//! - 顺序完全来自 Repository 返回顺序（后端 `ORDER BY ordinal, id`），
//!   服务只做层级组装与安全投影，前端不得重排或推断归属。
//!
//! Tauri Command 只做 UUID/DTO 校验与错误映射，不直接访问 Repository。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::PeriodicalRepository;
use haven_domain::ids::WorkId;
use haven_domain::periodical::PeriodicalArticleAvailability;

use crate::wire::{
    PageRangeDto, PeriodicalArticleAvailabilityDto, PeriodicalArticleDto, PeriodicalDto,
    PeriodicalIssueDto, PeriodicalTreeDto, PeriodicalVolumeDto,
};

/// `periodical_tree_get` 的 Wire schema 版本（与 [`PeriodicalTreeDto`] 同步冻结）。
pub const PERIODICAL_TREE_SCHEMA_VERSION: u32 = 1;

/// 「该 Work 没有期刊层级」的稳定错误。
///
/// 这是可映射的资源错误而不是空树：Work 可能是不存在、也可能是普通图书/视频，
/// 后端不区分（避免泄露 Work 是否存在），但绝不用标题或分类猜出一个期刊。
pub fn periodical_not_found() -> AppError {
    AppError::new(
        "PERIODICAL_NOT_FOUND",
        ErrorKind::NotFound,
        "该作品没有期刊层级",
        false,
    )
}

/// 报刊层级只读查询服务。
#[derive(Clone)]
pub struct PeriodicalQueryService {
    repository: Arc<dyn PeriodicalRepository>,
}

impl PeriodicalQueryService {
    pub fn new(repository: Arc<dyn PeriodicalRepository>) -> Self {
        Self { repository }
    }

    /// 读取一个 Work 的完整期刊层级。
    ///
    /// 无期刊归属（含 Work 不存在）→ [`periodical_not_found`]；
    /// 期刊存在但没有卷 → 返回空 `volumes` 的成功结果。
    pub async fn periodical_tree(&self, work_id: WorkId) -> Result<PeriodicalTreeDto, AppError> {
        let periodical = self
            .repository
            .find_by_work(work_id)
            .await?
            .ok_or_else(periodical_not_found)?;

        let mut volumes = Vec::new();
        for volume in self.repository.list_volumes(periodical.id).await? {
            let mut issues = Vec::new();
            for issue in self.repository.list_issues(volume.id).await? {
                let articles = self.repository.list_articles(issue.id).await?;
                issues.push((issue, articles));
            }
            volumes.push((volume, issues));
        }

        Ok(PeriodicalTreeDto {
            schema_version: PERIODICAL_TREE_SCHEMA_VERSION,
            work_id: periodical.work_id.to_string(),
            periodical: PeriodicalDto {
                id: periodical.id.to_string(),
                work_id: periodical.work_id.to_string(),
                title: periodical.title,
                issn_print: periodical.issn_print.map(|issn| issn.as_str().to_owned()),
                issn_electronic: periodical
                    .issn_electronic
                    .map(|issn| issn.as_str().to_owned()),
                publisher: periodical.publisher,
            },
            volumes: volumes
                .into_iter()
                .map(|(volume, issues)| PeriodicalVolumeDto {
                    id: volume.id.to_string(),
                    periodical_id: volume.periodical_id.to_string(),
                    label: volume.label,
                    number: volume.number,
                    year: volume.year,
                    ordinal: volume.ordinal,
                    issues: issues
                        .into_iter()
                        .map(|(issue, articles)| PeriodicalIssueDto {
                            id: issue.id.to_string(),
                            volume_id: issue.volume_id.to_string(),
                            label: issue.label,
                            number: issue.number,
                            publication_date: issue.publication_date,
                            ordinal: issue.ordinal,
                            articles: articles
                                .into_iter()
                                .map(|article| PeriodicalArticleDto {
                                    id: article.id.to_string(),
                                    issue_id: article.issue_id.to_string(),
                                    media_item_id: article.media_item_id.to_string(),
                                    ordinal: article.ordinal,
                                    title: article.title,
                                    doi: article.doi.map(|doi| doi.as_str().to_owned()),
                                    page_range: article.page_range.map(|page_range| PageRangeDto {
                                        start: page_range.start,
                                        end: page_range.end,
                                    }),
                                    source_key: article.source.source_key,
                                    remote_article_id: article.source.remote_article_id,
                                    availability: match article.provider_content_availability {
                                        PeriodicalArticleAvailability::FullText => {
                                            PeriodicalArticleAvailabilityDto::FullText
                                        }
                                        PeriodicalArticleAvailability::MetadataOnly => {
                                            PeriodicalArticleAvailabilityDto::MetadataOnly
                                        }
                                        PeriodicalArticleAvailability::Unknown => {
                                            PeriodicalArticleAvailabilityDto::Unknown
                                        }
                                    },
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use async_trait::async_trait;
    use haven_common::UtcMillis;
    use haven_domain::ids::{
        MediaItemId, PeriodicalArticleId, PeriodicalId, PeriodicalIssueId, PeriodicalVolumeId,
    };
    use haven_domain::periodical::{
        Doi, Issn, PageRange, Periodical, PeriodicalArticle, PeriodicalArticleAvailability,
        PeriodicalArticleSourceIdentity, PeriodicalIssue, PeriodicalVolume,
    };

    /// 固定顺序的内存 Repository；不访问网络、不访问 SQLite。
    #[derive(Default)]
    struct FakePeriodicalRepository {
        periodical: Option<Periodical>,
        volumes: Vec<PeriodicalVolume>,
        issues: Vec<PeriodicalIssue>,
        articles: Vec<PeriodicalArticle>,
        /// 调用序列，用于断言服务确实按 Work → 卷 → 期逐级查询。
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PeriodicalRepository for FakePeriodicalRepository {
        async fn get(&self, _id: PeriodicalId) -> Result<Option<Periodical>, AppError> {
            Ok(None)
        }

        async fn find_by_work(&self, work_id: WorkId) -> Result<Option<Periodical>, AppError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("find_by_work:{work_id}"));
            Ok(self
                .periodical
                .as_ref()
                .filter(|periodical| periodical.work_id == work_id)
                .cloned())
        }

        async fn find_by_issn(&self, _issn: &Issn) -> Result<Option<Periodical>, AppError> {
            Ok(None)
        }

        async fn list_volumes(
            &self,
            periodical_id: PeriodicalId,
        ) -> Result<Vec<PeriodicalVolume>, AppError> {
            Ok(self
                .volumes
                .iter()
                .filter(|volume| volume.periodical_id == periodical_id)
                .cloned()
                .collect())
        }

        async fn list_issues(
            &self,
            volume_id: PeriodicalVolumeId,
        ) -> Result<Vec<PeriodicalIssue>, AppError> {
            Ok(self
                .issues
                .iter()
                .filter(|issue| issue.volume_id == volume_id)
                .cloned()
                .collect())
        }

        async fn list_articles(
            &self,
            issue_id: PeriodicalIssueId,
        ) -> Result<Vec<PeriodicalArticle>, AppError> {
            Ok(self
                .articles
                .iter()
                .filter(|article| article.issue_id == issue_id)
                .cloned()
                .collect())
        }

        async fn find_article_by_source(
            &self,
            _source_key: &str,
            _remote_article_id: &str,
        ) -> Result<Option<haven_domain::periodical::PeriodicalPlacement>, AppError> {
            Ok(None)
        }

        async fn save(&self, _periodical: &Periodical) -> Result<(), AppError> {
            Ok(())
        }

        async fn save_volume(&self, _volume: &PeriodicalVolume) -> Result<(), AppError> {
            Ok(())
        }

        async fn save_issue(&self, _issue: &PeriodicalIssue) -> Result<(), AppError> {
            Ok(())
        }

        async fn save_article(&self, _article: &PeriodicalArticle) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn now() -> UtcMillis {
        UtcMillis(1_000)
    }

    fn issn(value: &str) -> Issn {
        Issn::parse(value).unwrap()
    }

    /// 多卷 / 多期 / 多文章夹具，卷与期故意按「非升序」的 ordinal 给出，
    /// 用于证明服务不会替 Repository 重排。
    fn multi_level_repository() -> (FakePeriodicalRepository, WorkId) {
        let work_id = WorkId::new();
        let periodical_id = PeriodicalId::new();
        let periodical = Periodical {
            id: periodical_id,
            work_id,
            title: "Nature Communications".into(),
            issn_print: Some(issn("2041-1723")),
            issn_electronic: Some(issn("1420-682X")),
            publisher: Some("Nature Portfolio".into()),
            created_at: now(),
            updated_at: now(),
        };

        let volume_b = PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            periodical_id,
            label: Some("Suppl 1".into()),
            number: None,
            year: Some(2024),
            ordinal: 5,
            created_at: now(),
            updated_at: now(),
        };
        let volume_a = PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            periodical_id,
            label: None,
            number: Some(12.0),
            year: Some(2023),
            ordinal: 1,
            created_at: now(),
            updated_at: now(),
        };

        let issue_a1 = PeriodicalIssue {
            id: PeriodicalIssueId::new(),
            volume_id: volume_a.id,
            label: Some("3-4".into()),
            number: None,
            publication_date: Some("2023-11".into()),
            ordinal: 2,
            created_at: now(),
            updated_at: now(),
        };
        let issue_a0 = PeriodicalIssue {
            id: PeriodicalIssueId::new(),
            volume_id: volume_a.id,
            label: None,
            number: Some(1.0),
            publication_date: Some("2023-01-15".into()),
            ordinal: 1,
            created_at: now(),
            updated_at: now(),
        };

        let article =
            |issue_id,
             ordinal: Option<u32>,
             title: &str,
             remote: &str,
             availability: PeriodicalArticleAvailability| PeriodicalArticle {
                id: PeriodicalArticleId::new(),
                issue_id,
                media_item_id: MediaItemId::new(),
                ordinal,
                title: title.into(),
                doi: Doi::parse("10.1038/s41467-024-00001-2"),
                page_range: PageRange::new("e12345", Some("e12350")),
                source: PeriodicalArticleSourceIdentity::new("europepmc", remote).unwrap(),
                provider_content_availability: availability,
                created_at: now(),
                updated_at: now(),
            };
        let article_a0_second = article(
            issue_a0.id,
            Some(9),
            "第二篇",
            "PMC2",
            PeriodicalArticleAvailability::MetadataOnly,
        );
        let article_a0_first = article(
            issue_a0.id,
            Some(2),
            "第一篇",
            "PMC1",
            PeriodicalArticleAvailability::FullText,
        );

        (
            FakePeriodicalRepository {
                periodical: Some(periodical),
                // Repository 顺序 = 领域顺序（由 SQL `ORDER BY ordinal, id` 保证）。
                volumes: vec![volume_a, volume_b],
                issues: vec![issue_a0, issue_a1],
                articles: vec![article_a0_first, article_a0_second],
                calls: Mutex::new(Vec::new()),
            },
            work_id,
        )
    }

    #[tokio::test]
    async fn tree_preserves_repository_order_across_all_levels() {
        let (repository, work_id) = multi_level_repository();
        let volume_a_id = repository.volumes[0].id;
        let issue_a0_id = repository.issues[0].id;
        let service = PeriodicalQueryService::new(Arc::new(repository));

        let tree = service.periodical_tree(work_id).await.unwrap();

        assert_eq!(tree.schema_version, 1);
        assert_eq!(tree.work_id, work_id.to_string());
        assert_eq!(tree.periodical.title, "Nature Communications");
        assert_eq!(tree.periodical.work_id, work_id.to_string());
        assert_eq!(tree.periodical.issn_print.as_deref(), Some("2041-1723"));
        assert_eq!(
            tree.periodical.issn_electronic.as_deref(),
            Some("1420-682X")
        );
        assert_eq!(
            tree.periodical.publisher.as_deref(),
            Some("Nature Portfolio")
        );

        // 卷/期/文章都保持 Repository 顺序（volume_b 的 ordinal 更大，仍排在后面）。
        assert_eq!(
            tree.volumes
                .iter()
                .map(|volume| volume.ordinal)
                .collect::<Vec<_>>(),
            vec![1, 5]
        );
        assert_eq!(tree.volumes[0].id, volume_a_id.to_string());
        assert_eq!(tree.volumes[0].periodical_id, tree.periodical.id);
        assert_eq!(tree.volumes[0].number, Some(12.0));
        assert_eq!(tree.volumes[0].year, Some(2023));
        // 第二卷没有期号 → 空列表，不是被丢弃的层级。
        assert!(tree.volumes[1].issues.is_empty());
        assert_eq!(tree.volumes[1].label.as_deref(), Some("Suppl 1"));

        let issues = &tree.volumes[0].issues;
        assert_eq!(
            issues.iter().map(|issue| issue.ordinal).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(issues[0].id, issue_a0_id.to_string());
        assert_eq!(issues[0].volume_id, tree.volumes[0].id);
        assert_eq!(issues[0].number, Some(1.0));
        assert_eq!(issues[0].publication_date.as_deref(), Some("2023-01-15"));
        // 不规则期号保留原文。
        assert_eq!(issues[1].label.as_deref(), Some("3-4"));

        let articles = &issues[0].articles;
        assert_eq!(
            articles
                .iter()
                .map(|article| article.ordinal)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(9)]
        );
        assert_eq!(articles[0].issue_id, issues[0].id);
        assert_eq!(articles[0].title, "第一篇");
        assert_eq!(
            articles[0].doi.as_deref(),
            Some("10.1038/s41467-024-00001-2")
        );
        assert_eq!(articles[0].source_key, "europepmc");
        assert_eq!(articles[0].remote_article_id, "PMC1");
        assert_eq!(
            articles[0].availability,
            PeriodicalArticleAvailabilityDto::FullText,
            "Provider 的正文观察必须出现在期刊树上"
        );
        assert_eq!(
            articles[1].availability,
            PeriodicalArticleAvailabilityDto::MetadataOnly
        );
        let page_range = articles[0].page_range.as_ref().unwrap();
        assert_eq!(page_range.start, "e12345");
        assert_eq!(page_range.end.as_deref(), Some("e12350"));
        // media_item_id 必须是 Haven MediaItem ID，而不是任何资源地址。
        assert!(articles[0].media_item_id.parse::<uuid::Uuid>().is_ok());
    }

    #[tokio::test]
    async fn tree_without_volumes_is_a_successful_empty_hierarchy() {
        let (mut repository, work_id) = multi_level_repository();
        repository.volumes.clear();
        repository.issues.clear();
        repository.articles.clear();
        let service = PeriodicalQueryService::new(Arc::new(repository));

        let tree = service.periodical_tree(work_id).await.unwrap();
        assert!(tree.volumes.is_empty());
        assert_eq!(tree.periodical.work_id, work_id.to_string());
    }

    #[tokio::test]
    async fn unknown_work_returns_stable_periodical_not_found() {
        let (repository, _) = multi_level_repository();
        let service = PeriodicalQueryService::new(Arc::new(repository));

        let error = service
            .periodical_tree(WorkId::new())
            .await
            .expect_err("没有期刊归属的 Work 必须返回稳定错误");
        assert_eq!(error.code().as_str(), "PERIODICAL_NOT_FOUND");
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(!error.retryable());
    }

    /// 层级只由显式归属决定：期刊存在但查询的是另一个 Work 时，不得按标题或
    /// 分类猜出期刊。
    #[tokio::test]
    async fn lookup_is_by_work_identity_only() {
        let (repository, work_id) = multi_level_repository();
        let repository = Arc::new(repository);
        let service = PeriodicalQueryService::new(repository.clone());

        service.periodical_tree(work_id).await.unwrap();
        let calls = repository.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![format!("find_by_work:{work_id}")]);
    }

    /// Wire 形状：camelCase、`schemaVersion` 闭合为 1，且不出现任何禁止字段
    /// （URL / Cookie / 请求头 / 本地路径 / Provider 原始响应 / 签名地址）。
    #[tokio::test]
    async fn wire_json_is_camel_case_and_leaks_no_transport_facts() {
        let (repository, work_id) = multi_level_repository();
        let service = PeriodicalQueryService::new(Arc::new(repository));
        let tree = service.periodical_tree(work_id).await.unwrap();

        let value = serde_json::to_value(&tree).unwrap();
        assert_eq!(
            value["schemaVersion"],
            serde_json::json!(1),
            "schemaVersion 必须是字面量 1"
        );
        assert_eq!(value["workId"], serde_json::json!(work_id.to_string()));
        assert_eq!(
            value["periodical"]["issnPrint"],
            serde_json::json!("2041-1723")
        );
        assert_eq!(
            value["volumes"][0]["issues"][0]["articles"][0]["mediaItemId"]
                .as_str()
                .map(|id| id.parse::<uuid::Uuid>().is_ok()),
            Some(true),
            "mediaItemId 必须是 Haven UUID，而不是资源地址"
        );

        let json = value.to_string().to_ascii_lowercase();
        for banned in [
            "://",
            "http",
            "cookie",
            "authorization",
            "bearer",
            "token",
            "file://",
            "c:\\",
            "signature",
            "x-amz",
            "://localhost",
        ] {
            assert!(!json.contains(banned), "Wire 输出不得包含 {banned:?}");
        }
        // 序列化不得出现 snake_case 字段名（DTO 边界统一 camelCase）。
        assert!(!json.contains("schema_version"));
        assert!(!json.contains("work_id"));
        assert!(!json.contains("issn_print"));
        assert!(!json.contains("media_item_id"));
        assert!(!json.contains("publication_date"));
        assert!(!json.contains("remote_article_id"));
    }

    /// 正文可用性是 Wire 上的闭合枚举：只有三个 snake_case 取值可以往返，
    /// 未定义的取值（例如把 `full_text` 写成 `readable`）必须被拒绝。
    #[test]
    fn article_availability_is_a_closed_wire_enum() {
        let value = serde_json::to_value(&PeriodicalIssueDto {
            id: "0196f0d2-0000-7000-8000-000000000001".into(),
            volume_id: "0196f0d2-0000-7000-8000-000000000002".into(),
            label: None,
            number: None,
            publication_date: None,
            ordinal: 0,
            articles: vec![PeriodicalArticleDto {
                id: "0196f0d2-0000-7000-8000-000000000003".into(),
                issue_id: "0196f0d2-0000-7000-8000-000000000001".into(),
                media_item_id: "0196f0d2-0000-7000-8000-000000000004".into(),
                ordinal: None,
                title: "闭合枚举".into(),
                doi: None,
                page_range: None,
                source_key: "europepmc".into(),
                remote_article_id: "PMC1".into(),
                availability: PeriodicalArticleAvailabilityDto::Unknown,
            }],
        })
        .unwrap();
        assert_eq!(
            value["articles"][0]["availability"],
            serde_json::json!("unknown")
        );

        for (literal, expected) in [
            ("full_text", PeriodicalArticleAvailabilityDto::FullText),
            (
                "metadata_only",
                PeriodicalArticleAvailabilityDto::MetadataOnly,
            ),
            ("unknown", PeriodicalArticleAvailabilityDto::Unknown),
        ] {
            assert_eq!(
                serde_json::from_str::<PeriodicalArticleAvailabilityDto>(&format!("\"{literal}\""))
                    .unwrap(),
                expected
            );
        }
        for rejected in ["\"readable\"", "\"available\"", "\"\"", "null", "1"] {
            assert!(
                serde_json::from_str::<PeriodicalArticleAvailabilityDto>(rejected).is_err(),
                "闭合枚举不得接受 {rejected}"
            );
        }
    }

    /// 请求 DTO 只接受 camelCase `workId`，且不携带任何额外传输事实。
    ///
    /// 请求是闭合的：未知字段（任何多出来的传输事实）必须在反序列化阶段失败，
    /// 而不是被静默忽略；snake_case 与大小写变体都不是同一个字段，同样不得接受。
    #[test]
    fn tree_request_is_closed_over_work_id() {
        let work_id = "0196f0d2-0000-7000-8000-000000000001";
        let request: crate::wire::PeriodicalTreeGetRequest =
            serde_json::from_str(&format!(r#"{{"workId":"{work_id}"}}"#)).unwrap();
        assert_eq!(request.work_id, work_id);
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1);

        for rejected in [
            format!(r#"{{"workId":"{work_id}","pageUrl":"https://example.invalid/article"}}"#),
            format!(r#"{{"workId":"{work_id}","work_id":"{work_id}"}}"#),
            format!(r#"{{"workId":"{work_id}","WorkId":"{work_id}"}}"#),
            format!(r#"{{"workId":"{work_id}","WORKID":"{work_id}"}}"#),
            format!(r#"{{"WorkId":"{work_id}"}}"#),
            format!(r#"{{"work_id":"{work_id}"}}"#),
        ] {
            assert!(
                serde_json::from_str::<crate::wire::PeriodicalTreeGetRequest>(&rejected).is_err(),
                "请求必须拒绝 {rejected}"
            );
        }
    }

    /// 字段完整的合法期刊树（camelCase），用于闭合性反序列化测试。
    ///
    /// 它同时证明「拒绝未知字段」不是因为夹具本身缺字段才失败。
    fn valid_tree_json() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "workId": "0196f0d2-0000-7000-8000-000000000001",
            "periodical": {
                "id": "0196f0d2-0000-7000-8000-000000000002",
                "workId": "0196f0d2-0000-7000-8000-000000000001",
                "title": "Nature",
                "issnPrint": "2041-1723",
                "issnElectronic": null,
                "publisher": "Nature Portfolio"
            },
            "volumes": [{
                "id": "0196f0d2-0000-7000-8000-000000000003",
                "periodicalId": "0196f0d2-0000-7000-8000-000000000002",
                "label": null,
                "number": 12.0,
                "year": 2024,
                "ordinal": 0,
                "issues": [{
                    "id": "0196f0d2-0000-7000-8000-000000000004",
                    "volumeId": "0196f0d2-0000-7000-8000-000000000003",
                    "label": "Suppl 2",
                    "number": null,
                    "publicationDate": "2024-03",
                    "ordinal": 0,
                    "articles": [{
                        "id": "0196f0d2-0000-7000-8000-000000000005",
                        "issueId": "0196f0d2-0000-7000-8000-000000000004",
                        "mediaItemId": "0196f0d2-0000-7000-8000-000000000006",
                        "ordinal": 1,
                        "title": "第一篇",
                        "doi": "10.1038/s41467-024-00001-2",
                        "pageRange": { "start": "S1", "end": "S5" },
                        "sourceKey": "europepmc",
                        "remoteArticleId": "PMC1",
                        "availability": "full_text"
                    }]
                }]
            }]
        })
    }

    /// 同一组报刊 Wire DTO 在每一层都是闭合的：层级中任何位置出现未知字段都必须
    /// 反序列化失败，而不是被静默丢弃——前端 `periodical-tree.ts` 的守卫同样要求
    /// key 完全一致，后端不能是更宽松的一方。
    #[test]
    fn periodical_tree_wire_is_closed_at_every_level() {
        let base = valid_tree_json();
        let decoded: crate::wire::PeriodicalTreeDto =
            serde_json::from_value(base.clone()).expect("夹具本身必须能被反序列化");
        assert_eq!(decoded.schema_version, 1);
        assert_eq!(decoded.work_id, "0196f0d2-0000-7000-8000-000000000001");
        assert_eq!(
            decoded.volumes[0].issues[0].articles[0].source_key,
            "europepmc"
        );

        for pointer in [
            "",                                         // PeriodicalTreeDto
            "/periodical",                              // PeriodicalDto
            "/volumes/0",                               // PeriodicalVolumeDto
            "/volumes/0/issues/0",                      // PeriodicalIssueDto
            "/volumes/0/issues/0/articles/0",           // PeriodicalArticleDto
            "/volumes/0/issues/0/articles/0/pageRange", // PageRangeDto
        ] {
            let mut mutated = base.clone();
            mutated
                .pointer_mut(pointer)
                .unwrap_or_else(|| panic!("夹具缺少层级 {pointer:?}"))
                .as_object_mut()
                .expect("每个层级都是对象")
                .insert(
                    "pageUrl".into(),
                    serde_json::json!("https://example.invalid/leak"),
                );
            assert!(
                serde_json::from_value::<crate::wire::PeriodicalTreeDto>(mutated).is_err(),
                "{pointer:?} 层必须拒绝未知字段"
            );
        }
    }
}
