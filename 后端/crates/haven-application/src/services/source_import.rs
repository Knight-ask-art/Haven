//! SourceImportService：来源候选入库（V2-B 实战批次）。
//!
//! 流程：候选（operationId+index 缓存的外部 ID）→ 来源详情 → work_upsert 去重入库
//! （契约 §36.1 去重键持久化于 work_source_refs）→ 返回真实 Work/MediaItem 身份。
//! 幂等：重复导入同一外部条目返回既有身份，不产生重复作品。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ResourceRepository, WorkRepository,
};
use haven_domain::entities::{Edition, MediaIndex, MediaItem, Resource, Work};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaType, ResourceType, WorkStatus, WorkType,
};
use haven_domain::ids::{MediaItemId, ResourceId, WorkId};

use crate::services::ports::SourceImportPorts;
use crate::services::source_registry::SourceRegistryService;
use crate::wire::ContentCategory;

/// 来源目录条目（application 视角；由 infrastructure 适配器填充）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCatalogEntry {
    pub external_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub type_name: Option<String>,
    /// 海报地址（http(s)；由流水线经受控代理注册）。
    pub pic: Option<String>,
    /// 首播放组内的集数（标签 + 播放地址）。
    pub episodes: Vec<(String, String)>,
    /// 简介（已清洗去 HTML）。
    pub content: Option<String>,
    /// 导演（已清洗 / 分隔）。
    pub director: Option<String>,
    /// 主演（已清洗 / 分隔）。
    pub actor: Option<String>,
    /// 已获取到受控存储的本地文件（OPDS 书籍；CMS10 流媒体为 None）。
    pub local_file: Option<LocalAcquiredFile>,
}

/// 已落盘到登记存储位置的文件（受控资源；路径不进 IPC）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAcquiredFile {
    /// 登记存储位置 ID（字符串形态；入库时解析回强类型）。
    pub storage_location_id: String,
    /// 存储位置内的对象相对路径。
    pub object_rel_path: String,
    pub size_bytes: u64,
    pub mime: String,
}

/// 来源目录提供方端口（infrastructure 的 CMS10 客户端实现）。
#[async_trait]
pub trait SourceCatalogProvider: Send + Sync {
    async fn detail(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError>;
    /// 关键词搜索（enrichment 标题匹配用）；来源不支持时返回空集。
    async fn search(
        &self,
        source_id: &str,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceCatalogEntry>, AppError> {
        let _ = (source_id, endpoint, query, limit);
        Ok(Vec::new())
    }
}

/// 来源入库服务。
#[derive(Clone)]
pub struct SourceImportService {
    ports: Arc<dyn SourceImportPorts>,
    registry: SourceRegistryService,
    catalog: Arc<dyn SourceCatalogProvider>,
}

/// 入库结果：作品与首个可消费媒体条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedWork {
    pub work_id: WorkId,
    pub media_item_id: MediaItemId,
}

const SOURCE_PROVIDER: &str = "cms10";
/// OPDS 家族入库时 work_source_refs 的 provider 值。
pub const OPDS_SOURCE_PROVIDER: &str = "opds";

/// 候选卡片 work_id 前缀（非 UUID 形状即表明"未入库候选"，不得冒充已入库作品）。
pub const CMS10_CANDIDATE_PREFIX: &str = "cms10-candidate-";
/// OPDS 候选句柄前缀：`opds-candidate-<sourceId>\u{1}<entryUrl>`。
pub const OPDS_CANDIDATE_PREFIX: &str = "opds-candidate-";

impl SourceImportService {
    pub fn new(
        ports: Arc<dyn SourceImportPorts>,
        registry: SourceRegistryService,
        catalog: Arc<dyn SourceCatalogProvider>,
    ) -> Self {
        Self {
            ports,
            registry,
            catalog,
        }
    }

    /// 导入（幂等）：已存在引用时直接返回既有身份。
    pub async fn import_cms10_candidate(
        &self,
        external_id: &str,
    ) -> Result<ImportedWork, AppError> {
        let external_id = external_id.trim();
        if external_id.is_empty() || external_id.len() > 64 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "外部条目 ID 非法",
                false,
            ));
        }
        if let Some(work_id) =
            WorkRepository::id_for_source_ref(&*self.ports, SOURCE_PROVIDER, external_id).await?
        {
            return self.existing_identity(work_id).await;
        }

        // 端点必须已配置；未配置属于组合缺口，稳定失败而非猜测端点。
        let endpoint = self
            .registry
            .endpoint(SOURCE_PROVIDER)
            .await?
            .ok_or_else(|| {
                AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "该来源尚未配置端点",
                    false,
                )
            })?;
        let entry = self
            .catalog
            .detail(SOURCE_PROVIDER, &endpoint, external_id)
            .await?;

        let now = UtcMillis::now();
        let media_type = derive_media_type(entry.type_name.as_deref(), entry.episodes.len());

        // 海报经受控图片代理注册（契约 §36 C1）：外部 URL 不进 IPC 或 Work
        // 业务事实；仅落到受控技术映射，业务引用只保存 haven://artwork/<uuid>。
        let poster = match entry.pic.as_deref() {
            Some(url) => {
                match haven_domain::contracts::ImageProxyRepository::register(
                    &*self.ports,
                    SOURCE_PROVIDER,
                    url,
                )
                .await
                {
                    Ok(id) => Some(haven_domain::entities::ArtworkRef {
                        kind: haven_domain::entities::ArtworkKind::Poster,
                        uri: format!("haven://artwork/{id}"),
                        provider: Some(SOURCE_PROVIDER.to_string()),
                    }),
                    Err(_) => None,
                }
            }
            None => None,
        };

        let work = Work {
            id: WorkId::new(),
            canonical_title: entry.title.clone(),
            original_title: None,
            sort_title: None,
            description: entry.content.clone(),
            work_type: WorkType::Standalone,
            release_year: entry.year,
            language: None,
            director: entry.director.clone(),
            actor: entry.actor.clone(),
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: haven_domain::entities::ArtworkSet {
                poster,
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
        };
        WorkRepository::save(&*self.ports, &work).await?;
        WorkRepository::save_source_ref(&*self.ports, SOURCE_PROVIDER, external_id, work.id)
            .await?;

        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: entry.title.clone(),
            subtitle: None,
            edition_type: media_type,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        EditionRepository::save(&*self.ports, &edition).await?;

        let mut first_item: Option<MediaItemId> = None;
        for (index, (label, url)) in entry.episodes.iter().enumerate() {
            let ordinal = index as u32 + 1;
            let index_kind = if media_type == MediaType::Movie {
                MediaIndex::Movie
            } else {
                MediaIndex::Episode {
                    season: None,
                    episode: parse_episode_number(label, ordinal),
                }
            };
            let item = MediaItem {
                id: haven_domain::ids::MediaItemId::new(),
                edition_id: edition.id,
                parent_id: None,
                media_type,
                title: label.clone(),
                index: index_kind,
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: haven_domain::enums::MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            };
            MediaItemRepository::save(&*self.ports, &item).await?;
            let resource = Resource {
                id: ResourceId::new(),
                media_item_id: item.id,
                resource_type: stream_resource_type(url),
                // 资源级来源追踪暂不需要；作品级去重由 work_source_refs 承担。
                source_id: None,
                storage_location_id: None,
                locator: haven_domain::entities::ResourceLocator::Http { url: url.clone() },
                mime_type: Some(stream_mime(url).to_owned()),
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            };
            ResourceRepository::save(&*self.ports, &resource).await?;
            first_item.get_or_insert(item.id);
        }

        let Some(media_item_id) = first_item else {
            return Err(source_unavailable("采集站条目没有可播放地址"));
        };
        Ok(ImportedWork {
            work_id: work.id,
            media_item_id,
        })
    }

    /// 候选句柄路由导入：OPDS 前缀走书源链路；其余按 CMS10 处理（兼容旧缓存）。
    pub async fn import_candidate(&self, handle: &str) -> Result<ImportedWork, AppError> {
        let handle = handle.trim();
        if let Some(rest) = handle.strip_prefix(OPDS_CANDIDATE_PREFIX) {
            let Some((source_id, external_id)) = rest.split_once('\u{1}') else {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "候选句柄非法",
                    false,
                ));
            };
            return self.import_opds_candidate(source_id, external_id).await;
        }
        let inner = handle
            .strip_prefix(CMS10_CANDIDATE_PREFIX)
            .unwrap_or(handle);
        self.import_cms10_candidate(inner).await
    }

    /// OPDS 书籍入库：条目页抓取 → EPUB 受控落盘（Provider 内完成）→ 四仓写入。
    /// 幂等键：`opds` provider + `<sourceId>\u{1}<entryUrl>`。
    pub async fn import_opds_candidate(
        &self,
        source_id: &str,
        entry_url: &str,
    ) -> Result<ImportedWork, AppError> {
        let source_id = source_id.trim();
        let entry_url = entry_url.trim();
        if source_id.is_empty() || source_id.len() > 64 || !entry_url.starts_with("http") {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "书籍候选标识非法",
                false,
            ));
        }
        if entry_url.len() > 512 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "书籍候选地址超长",
                false,
            ));
        }
        let dedupe_external = format!("{source_id}\u{1}{entry_url}");
        if let Some(work_id) =
            WorkRepository::id_for_source_ref(&*self.ports, OPDS_SOURCE_PROVIDER, &dedupe_external)
                .await?
        {
            return self.existing_identity(work_id).await;
        }

        let endpoint = self.registry.endpoint(source_id).await?.ok_or_else(|| {
            AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "该来源尚未配置端点",
                false,
            )
        })?;
        let entry = self.catalog.detail(source_id, &endpoint, entry_url).await?;
        let acquired = entry.local_file.clone().ok_or_else(|| {
            AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "该条目没有可离线的 EPUB 文件",
                true,
            )
        })?;

        let now = UtcMillis::now();
        let media_type = MediaType::Book;

        let poster = match entry.pic.as_deref() {
            Some(url) => {
                match haven_domain::contracts::ImageProxyRepository::register(
                    &*self.ports,
                    OPDS_SOURCE_PROVIDER,
                    url,
                )
                .await
                {
                    Ok(id) => Some(haven_domain::entities::ArtworkRef {
                        kind: haven_domain::entities::ArtworkKind::Poster,
                        uri: format!("haven://artwork/{id}"),
                        provider: Some(OPDS_SOURCE_PROVIDER.to_owned()),
                    }),
                    Err(_) => None,
                }
            }
            None => None,
        };

        let work = Work {
            id: WorkId::new(),
            canonical_title: entry.title.clone(),
            original_title: None,
            sort_title: None,
            description: entry.content.clone(),
            work_type: WorkType::Standalone,
            release_year: entry.year,
            language: None,
            director: entry.director.clone(),
            actor: entry.actor.clone(),
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: haven_domain::entities::ArtworkSet {
                poster,
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
        };
        WorkRepository::save(&*self.ports, &work).await?;
        WorkRepository::save_source_ref(
            &*self.ports,
            OPDS_SOURCE_PROVIDER,
            &dedupe_external,
            work.id,
        )
        .await?;

        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: entry.title.clone(),
            subtitle: None,
            edition_type: media_type,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        EditionRepository::save(&*self.ports, &edition).await?;

        let object_id = acquired
            .object_rel_path
            .rsplit('/')
            .next()
            .unwrap_or(&acquired.object_rel_path)
            .to_owned();
        let item = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type,
            title: entry.title.clone(),
            index: haven_domain::entities::MediaIndex::Custom {
                label: "正文".to_owned(),
                ordinal: Some(1.0),
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: haven_domain::enums::MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        MediaItemRepository::save(&*self.ports, &item).await?;

        let location_id: haven_domain::ids::StorageLocationId =
            acquired.storage_location_id.parse().map_err(|_| {
                AppError::new(
                    "INTERNAL_ERROR",
                    ErrorKind::Internal,
                    "存储位置身份解析失败",
                    false,
                )
            })?;
        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: item.id,
            resource_type: ResourceType::PublicationFile,
            source_id: None,
            storage_location_id: Some(location_id),
            locator: haven_domain::entities::ResourceLocator::StorageObject {
                provider_id: location_id,
                object_id,
                path_hint: Some(acquired.object_rel_path),
            },
            mime_type: Some(acquired.mime),
            size: Some(acquired.size_bytes),
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        };
        ResourceRepository::save(&*self.ports, &resource).await?;

        Ok(ImportedWork {
            work_id: work.id,
            media_item_id: item.id,
        })
    }

    /// enrichment 标题精确匹配（契约 §36.8）：CMS10 搜索后取标题完全相等的首条。
    /// 未配置端点/未命中返回 Ok(None)——不是错误，只是没有匹配。
    pub async fn match_cms10_by_title(
        &self,
        title: &str,
    ) -> Result<Option<crate::services::source_import::ImportedWork>, AppError> {
        let Some(endpoint) = self.registry.endpoint(SOURCE_PROVIDER).await? else {
            return Ok(None);
        };
        let candidates = self
            .catalog
            .search(SOURCE_PROVIDER, &endpoint, title, 10)
            .await?;
        let matched = candidates
            .into_iter()
            .find(|entry| entry.title == title)
            .map(|entry| entry.external_id);
        match matched {
            Some(external_id) => self.import_cms10_candidate(&external_id).await.map(Some),
            None => Ok(None),
        }
    }
    async fn existing_identity(&self, work_id: WorkId) -> Result<ImportedWork, AppError> {
        let editions = EditionRepository::list_by_work(&*self.ports, work_id).await?;
        for edition in editions {
            let items = MediaItemRepository::list_by_edition(&*self.ports, edition.id).await?;
            if let Some(item) = items.into_iter().next() {
                return Ok(ImportedWork {
                    work_id,
                    media_item_id: item.id,
                });
            }
        }
        Err(AppError::new(
            "DATABASE_ERROR",
            ErrorKind::Database,
            "来源引用指向的作品缺少可消费单元",
            true,
        ))
    }
}

/// 分类名 → 媒介类型：含"电影"为电影；单集亦按电影处理，其余按剧集。
fn derive_media_type(type_name: Option<&str>, episode_count: usize) -> MediaType {
    if type_name.is_some_and(|name| name.contains("电影")) || episode_count <= 1 {
        MediaType::Movie
    } else {
        MediaType::Series
    }
}

/// 从"第01集"/"第0001集"类标签提取集号；去前导零，无数字时退化为序号。
fn parse_episode_number(label: &str, fallback: u32) -> u32 {
    let digits: String = label
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return fallback;
    }
    // 去前导零，"0001" → 1
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return 1;
    }
    trimmed.parse().unwrap_or(fallback)
}

fn stream_resource_type(url: &str) -> ResourceType {
    if url.contains(".m3u8") {
        ResourceType::HlsStream
    } else {
        ResourceType::VideoStream
    }
}

fn stream_mime(url: &str) -> &'static str {
    if url.contains(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp4"
    }
}

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

/// 分类推导的辅助导出（供测试断言 ContentCategory 投影一致性）。
#[allow(dead_code)]
fn category_of(media_type: MediaType) -> ContentCategory {
    match media_type {
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
            ContentCategory::Video
        }
        _ => ContentCategory::Video,
    }
}
