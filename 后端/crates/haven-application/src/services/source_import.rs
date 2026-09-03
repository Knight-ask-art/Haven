//! SourceImportService：来源候选入库（V2-B 实战批次）。
//!
//! 流程：候选（operationId+index 缓存的外部 ID）→ 来源详情 → work_upsert 去重入库
//! （契约 §36.1 去重键持久化于 work_source_refs）→ 返回真实 Work/MediaItem 身份。
//! 幂等：重复导入同一外部条目返回既有身份，不产生重复作品。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{EditionRepository, MediaItemRepository, WorkRepository};
use haven_domain::entities::{Edition, MediaIndex, MediaItem, Resource, Work};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaType, ResourceType, WorkStatus, WorkType,
};
use haven_domain::ids::{MediaItemId, ResourceId, SourceId, WorkId};
use uuid::Uuid;

use crate::services::ports::{SourceImportPorts, UnitOfWork};
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
    ///
    /// 搜索详情不得填充此字段。它只由显式的离线获取流程产生；保留该字段
    /// 是为了兼容已有的本地导入适配器，新的远端来源应使用 `remote`。
    pub local_file: Option<LocalAcquiredFile>,
    /// 固定来源导入时的明确媒介类型。旧的 CMS10/OPDS 目录继续使用
    /// `type_name` + episodes 推导，在线漫画/文章不能依赖字符串猜测。
    pub media_type: Option<MediaType>,
    /// 远端正文的受控身份。此结构只在 Application 内部流转，不能直接进入
    /// 普通 Wire DTO；下载/在线 Session 再由后端根据 source key 解析。
    pub remote: Option<RemoteContentRef>,
}

/// 远端正文引用。只保存来源 key 和 Provider 自己的条目标识，不保存 URL、
/// Cookie 或任意请求头。`remote_id` 的格式由对应来源 Provider 在消费时校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteContentRef {
    pub source_key: String,
    pub remote_id: String,
    pub media_type: MediaType,
    pub mime_type: Option<String>,
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
    uow: Arc<dyn UnitOfWork>,
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
/// OPDS 候选句柄前缀：`opds-candidate-<sourceId>\u{1}<encoded-entry-identity>`。
///
/// 条目页身份以百分号编码形式存在于操作缓存句柄中，前端只能把它当作
/// opaque candidate 使用；只有服务端在来源校验通过后才会解码并重新验证。
pub const OPDS_CANDIDATE_PREFIX: &str = "opds-candidate-";
/// 当前唯一开放“搜索后可导入”的 OPDS 来源。自定义 OPDS 仍可搜索，
/// 但没有受控正文获取 Provider，必须在后端入口明确拒绝导入。
pub const OPDS_GUTENBERG_SOURCE_ID: &str = "opds_gutenberg";
/// 可导入正文候选的 opaque 句柄前缀。句柄只在搜索操作缓存与导入命令之间流转，
/// 不表示可由前端直接访问的 URL。
pub const CONTENT_CANDIDATE_PREFIX: &str = "content-candidate-";
/// M3U is a user-configured video collection.  It uses the same opaque
/// candidate envelope as the fixed online-content providers, but its payload
/// is a provider-owned `(display title, stream URL)` pair and is only decoded
/// at this application boundary.
pub const M3U_SOURCE_ID: &str = "m3u";

impl SourceImportService {
    pub fn new(
        ports: Arc<dyn SourceImportPorts>,
        uow: Arc<dyn UnitOfWork>,
        registry: SourceRegistryService,
        catalog: Arc<dyn SourceCatalogProvider>,
    ) -> Self {
        Self {
            ports,
            uow,
            registry,
            catalog,
        }
    }

    /// 将一次来源导入的全部业务内容交给单一 SQLite 事务提交。
    /// `source_ref` 与作品内容必须同生共灭，避免部分导入永久占用去重键。
    fn persist_import(
        &self,
        provider: &str,
        external_id: &str,
        work: &Work,
        edition: &Edition,
        items: &[MediaItem],
        resources: &[Resource],
    ) -> Result<(), AppError> {
        self.uow
            .run_source_import(provider, external_id, work, edition, items, resources)
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

        let mut items = Vec::with_capacity(entry.episodes.len());
        let mut resources = Vec::with_capacity(entry.episodes.len());
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
            items.push(item);
            resources.push(resource);
        }

        let Some(media_item_id) = items.first().map(|item| item.id) else {
            return Err(source_unavailable("采集站条目没有可播放地址"));
        };
        self.persist_import(
            SOURCE_PROVIDER,
            external_id,
            &work,
            &edition,
            &items,
            &resources,
        )?;
        Ok(ImportedWork {
            work_id: work.id,
            media_item_id,
        })
    }

    /// 候选句柄路由导入。
    ///
    /// 候选句柄必须带有明确的 opaque 前缀。搜索型 metadata Provider（例如
    /// `gutenberg`）会返回 `metadata-candidate-*`，它们没有受控正文获取链路，
    /// 因此必须在这里明确拒绝，不能回退到 CMS10 或把一个来源的候选误交给
    /// 另一个 Provider。旧版 CMS10 候选仍通过显式 `cms10-candidate-` 前缀兼容。
    pub async fn import_candidate(&self, handle: &str) -> Result<ImportedWork, AppError> {
        let handle = handle.trim();
        if let Some(rest) = handle.strip_prefix(OPDS_CANDIDATE_PREFIX) {
            let Some((source_id, encoded_external_id)) = rest.split_once('\u{1}') else {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "候选句柄非法",
                    false,
                ));
            };
            let source_id = source_id.trim();
            if source_id.is_empty() || source_id.len() > 64 || has_control_character(source_id) {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "候选句柄非法",
                    false,
                ));
            }
            let external_id = decode_candidate_component(encoded_external_id)?;
            return self.import_opds_candidate(source_id, &external_id).await;
        }
        if let Some(rest) = handle.strip_prefix(CONTENT_CANDIDATE_PREFIX) {
            let Some((source_id, encoded_external_id)) = rest.split_once('-') else {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "正文候选句柄非法",
                    false,
                ));
            };
            let external_id = if source_id == M3U_SOURCE_ID {
                decode_candidate_component_with_separator(encoded_external_id)?
            } else {
                decode_candidate_component(encoded_external_id)?
            };
            if source_id == M3U_SOURCE_ID {
                return self.import_m3u_candidate(&external_id).await;
            }
            return self.import_content_candidate(source_id, &external_id).await;
        }
        if let Some(external_id) = handle.strip_prefix(CMS10_CANDIDATE_PREFIX) {
            return self.import_cms10_candidate(external_id).await;
        }
        Err(AppError::new(
            "SOURCE_IMPORT_UNSUPPORTED",
            ErrorKind::Unsupported,
            "该搜索候选暂不支持导入媒体库",
            false,
        ))
    }

    /// 导入固定在线正文来源。
    ///
    /// 这里严格只登记元数据和 `SourceObject` 远端身份，不获取正文、不创建
    /// Offline Resource，也不写入用户下载目录。正文获取只能由显式下载任务
    /// 或受控 Remote Session 触发。
    pub async fn import_content_candidate(
        &self,
        source_id: &str,
        external_id: &str,
    ) -> Result<ImportedWork, AppError> {
        if !matches!(source_id, "mangadex" | "arxiv" | "europepmc" | "wikisource") {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "该来源不支持正文导入",
                false,
            ));
        }
        let external_id = external_id.trim();
        validate_remote_candidate_id(source_id, external_id)?;
        if let Some(work_id) =
            WorkRepository::id_for_source_ref(&*self.ports, source_id, external_id).await?
        {
            return self.existing_identity(work_id).await;
        }

        // 固定公开来源不使用用户端点；传入空端点只作为 trait 的兼容参数，
        // OnlineCatalogProvider 会按 source_id 选择源码内固定主机。
        let entry = self.catalog.detail(source_id, "", external_id).await?;
        let remote = entry.remote.clone().ok_or_else(|| {
            AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "该来源暂时没有可用的远端正文",
                true,
            )
        })?;
        self.import_remote_entry(source_id, external_id, entry, remote)
            .await
    }

    /// Import one M3U entry without ever asking the catalog provider to fetch
    /// the stream.  The playlist endpoint is fetched during search; this
    /// method receives only the opaque title/URL pair from the operation cache
    /// and persists the URL as a server-side HTTP locator.  The frontend never
    /// sees that locator and playback still goes through `stream_open`.
    async fn import_m3u_candidate(&self, external_id: &str) -> Result<ImportedWork, AppError> {
        let (title, url) = external_id
            .split_once('\u{1}')
            .ok_or_else(invalid_m3u_candidate)?;
        let title = title.trim();
        let url = url.trim();
        validate_m3u_stream_url(url)?;
        if title.is_empty() || title.len() > 240 || has_control_character(title) {
            return Err(invalid_m3u_candidate());
        }
        let dedupe_key = format!("{title}\u{1}{url}");
        if let Some(work_id) =
            WorkRepository::id_for_source_ref(&*self.ports, M3U_SOURCE_ID, &dedupe_key).await?
        {
            return self.existing_identity(work_id).await;
        }

        let now = UtcMillis::now();
        let work = Work {
            id: WorkId::new(),
            canonical_title: title.to_owned(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let media_type = MediaType::Series;
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: title.to_owned(),
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
        let item = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Episode,
            title: title.to_owned(),
            index: MediaIndex::Episode {
                season: None,
                episode: 1,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: haven_domain::enums::MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: item.id,
            resource_type: stream_resource_type(url),
            source_id: None,
            storage_location_id: None,
            locator: haven_domain::entities::ResourceLocator::Http {
                url: url.to_owned(),
            },
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
        // Keep the M3U path on the same transaction boundary as CMS10 and the
        // fixed remote providers.  A failed resource insert must not leave a
        // Work/source-ref pair that makes the next import look idempotently
        // complete while its playable unit is missing.
        self.persist_import(
            M3U_SOURCE_ID,
            &dedupe_key,
            &work,
            &edition,
            std::slice::from_ref(&item),
            std::slice::from_ref(&resource),
        )?;
        Ok(ImportedWork {
            work_id: work.id,
            media_item_id: item.id,
        })
    }

    async fn import_remote_entry(
        &self,
        source_key: &str,
        external_id: &str,
        entry: SourceCatalogEntry,
        remote: RemoteContentRef,
    ) -> Result<ImportedWork, AppError> {
        if remote.source_key != source_key
            || entry
                .media_type
                .is_some_and(|media_type| media_type != remote.media_type)
        {
            return Err(AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "来源远端身份无效",
                true,
            ));
        }
        let resource_type = match remote.media_type {
            MediaType::Comic => ResourceType::ComicArchive,
            MediaType::Article
                if remote
                    .mime_type
                    .as_deref()
                    .is_some_and(|m| m.contains("pdf")) =>
            {
                ResourceType::PublicationFile
            }
            MediaType::Article => ResourceType::ArticleSnapshot,
            MediaType::Book => ResourceType::PublicationFile,
            _ => ResourceType::LocalFile,
        };
        validate_remote_source_object(source_key, resource_type, &remote.remote_id)?;
        let source_uuid = stable_source_id(source_key)?;
        let now = UtcMillis::now();
        let poster = match entry.pic.as_deref() {
            Some(url) => match haven_domain::contracts::ImageProxyRepository::register(
                &*self.ports,
                source_key,
                url,
            )
            .await
            {
                Ok(id) => Some(haven_domain::entities::ArtworkRef {
                    kind: haven_domain::entities::ArtworkKind::Poster,
                    uri: format!("haven://artwork/{id}"),
                    provider: Some(source_key.to_owned()),
                }),
                Err(_) => None,
            },
            None => None,
        };
        let work = Work {
            id: WorkId::new(),
            canonical_title: entry.title.clone(),
            original_title: None,
            sort_title: None,
            description: entry.content.clone(),
            work_type: if remote.media_type == MediaType::Article {
                WorkType::Article
            } else {
                WorkType::Standalone
            },
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
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: entry.title.clone(),
            subtitle: None,
            edition_type: remote.media_type,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: entry.content.clone(),
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };

        let item = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: remote.media_type,
            title: entry.title,
            index: match remote.media_type {
                MediaType::Comic => MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
                MediaType::Article => MediaIndex::Article { ordinal: Some(1) },
                _ => MediaIndex::Custom {
                    label: "正文".to_owned(),
                    ordinal: Some(1.0),
                },
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: haven_domain::enums::MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };

        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: item.id,
            resource_type,
            source_id: Some(source_uuid),
            storage_location_id: None,
            locator: haven_domain::entities::ResourceLocator::SourceObject {
                source_id: source_uuid,
                remote_id: remote.remote_id,
            },
            mime_type: remote.mime_type,
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
        self.persist_import(
            source_key,
            external_id,
            &work,
            &edition,
            std::slice::from_ref(&item),
            std::slice::from_ref(&resource),
        )?;
        Ok(ImportedWork {
            work_id: work.id,
            media_item_id: item.id,
        })
    }

    #[allow(dead_code)]
    async fn import_local_entry(
        &self,
        source_id: &str,
        external_id: &str,
        entry: SourceCatalogEntry,
        acquired: LocalAcquiredFile,
    ) -> Result<ImportedWork, AppError> {
        let media_type = entry.media_type.ok_or_else(|| {
            AppError::new(
                "INTERNAL_ERROR",
                ErrorKind::Internal,
                "来源正文缺少媒介类型",
                false,
            )
        })?;
        validate_local_object_path(&acquired.object_rel_path)?;
        let location_id: haven_domain::ids::StorageLocationId =
            acquired.storage_location_id.parse().map_err(|_| {
                AppError::new(
                    "INTERNAL_ERROR",
                    ErrorKind::Internal,
                    "存储位置身份解析失败",
                    false,
                )
            })?;
        let now = UtcMillis::now();
        let poster = match entry.pic.as_deref() {
            Some(url) => match haven_domain::contracts::ImageProxyRepository::register(
                &*self.ports,
                source_id,
                url,
            )
            .await
            {
                Ok(id) => Some(haven_domain::entities::ArtworkRef {
                    kind: haven_domain::entities::ArtworkKind::Poster,
                    uri: format!("haven://artwork/{id}"),
                    provider: Some(source_id.to_owned()),
                }),
                Err(_) => None,
            },
            None => None,
        };
        let work = Work {
            id: WorkId::new(),
            canonical_title: entry.title.clone(),
            original_title: None,
            sort_title: None,
            description: entry.content.clone(),
            work_type: if media_type == MediaType::Article {
                WorkType::Article
            } else {
                WorkType::Standalone
            },
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
            description: entry.content.clone(),
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };

        let item = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type,
            title: entry.title,
            index: match media_type {
                MediaType::Comic => MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
                MediaType::Article => MediaIndex::Article { ordinal: Some(1) },
                _ => MediaIndex::Custom {
                    label: "正文".to_owned(),
                    ordinal: Some(1.0),
                },
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: haven_domain::enums::MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };

        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: item.id,
            resource_type: local_resource_type(media_type, &acquired.mime),
            // Source registry IDs (for example `mangadex`) are not internal
            // UUID-backed `SourceId` values.  Keep this optional field empty;
            // the durable provenance/deduplication key is `work_source_refs`.
            source_id: None,
            storage_location_id: Some(location_id),
            locator: haven_domain::entities::ResourceLocator::StorageObject {
                provider_id: location_id,
                object_id: acquired
                    .object_rel_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&acquired.object_rel_path)
                    .to_owned(),
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
        self.persist_import(
            source_id,
            external_id,
            &work,
            &edition,
            std::slice::from_ref(&item),
            std::slice::from_ref(&resource),
        )?;

        Ok(ImportedWork {
            work_id: work.id,
            media_item_id: item.id,
        })
    }

    /// OPDS 书籍入库：只抓取条目元数据并登记远端身份。
    /// 幂等键：`opds` provider + `<sourceId>\u{1}<entryUrl>`。
    ///
    /// `entry_url` 只允许由后端候选句柄解码或内部 Provider 传入，不能由
    /// 前端任意提交；Provider 仍会按来源的固定 HTTPS/Host 策略再次验证。
    pub async fn import_opds_candidate(
        &self,
        source_id: &str,
        entry_url: &str,
    ) -> Result<ImportedWork, AppError> {
        let source_id = source_id.trim();
        let entry_url = entry_url.trim();
        if source_id.is_empty()
            || source_id.len() > 64
            || has_control_character(source_id)
            || !(entry_url.starts_with("https://") || entry_url.starts_with("http://"))
            || has_control_character(entry_url)
        {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "书籍候选标识非法",
                false,
            ));
        }
        if source_id != OPDS_GUTENBERG_SOURCE_ID {
            return Err(AppError::new(
                "SOURCE_IMPORT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "该 OPDS 来源目前仅支持搜索，正文导入尚未开放",
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
        let remote = entry.remote.clone().ok_or_else(|| {
            AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "该条目没有可用的远端 EPUB 身份",
                true,
            )
        })?;
        if remote.source_key != source_id {
            return Err(AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "书籍远端身份与来源不一致",
                true,
            ));
        }
        stable_source_id(source_id)?;
        self.import_remote_entry(source_id, &dedupe_external, entry, remote)
            .await
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
    match stream_url_kind(url) {
        StreamUrlKind::Hls => ResourceType::HlsStream,
        StreamUrlKind::Dash => ResourceType::DashStream,
        StreamUrlKind::Video => ResourceType::VideoStream,
    }
}

fn stream_mime(url: &str) -> &'static str {
    match stream_url_kind(url) {
        StreamUrlKind::Hls => "application/vnd.apple.mpegurl",
        StreamUrlKind::Dash => "application/dash+xml",
        StreamUrlKind::Video => "video/mp4",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamUrlKind {
    Video,
    Hls,
    Dash,
}

/// Classify a stream from its URL path only. Query strings and fragments are
/// deliberately ignored so a token such as `?format=.m3u8` cannot change the
/// resource type, while case differences in the actual path remain harmless.
pub(crate) fn stream_url_kind(url: &str) -> StreamUrlKind {
    let Some((_, remainder)) = url.split_once("://") else {
        return StreamUrlKind::Video;
    };
    let Some(path_start) = remainder.find(['/', '?', '#']) else {
        return StreamUrlKind::Video;
    };
    // A slash is the only delimiter that begins the URL path. If the first
    // delimiter is a query or fragment, any later slash belongs to that query
    // and must not influence stream classification.
    if remainder.as_bytes().get(path_start) != Some(&b'/') {
        return StreamUrlKind::Video;
    };
    let path = remainder[path_start..]
        .split_once(['?', '#'])
        .map_or(&remainder[path_start..], |(path, _)| path)
        .to_ascii_lowercase();
    if path.ends_with(".m3u8") || path.ends_with(".m3u") {
        StreamUrlKind::Hls
    } else if path.ends_with(".mpd") {
        StreamUrlKind::Dash
    } else {
        StreamUrlKind::Video
    }
}

fn has_control_character(value: &str) -> bool {
    value.chars().any(|ch| ch == '\0' || ch.is_control())
}

/// M3U entries are stored in the candidate cache as a title + separator + URL
/// pair. The separator itself is an internal framing byte and is allowed only
/// for this one decoding path; all other control characters remain rejected.
fn decode_candidate_component_with_separator(value: &str) -> Result<String, AppError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(invalid_m3u_candidate());
        }
        let high = hex_value(bytes[index + 1]).ok_or_else(invalid_m3u_candidate)?;
        let low = hex_value(bytes[index + 2]).ok_or_else(invalid_m3u_candidate)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid_m3u_candidate())?;
    if decoded.is_empty() || decoded.chars().any(|ch| ch.is_control() && ch != '\u{1}') {
        return Err(invalid_m3u_candidate());
    }
    Ok(decoded)
}

/// Validate the URL carried by an M3U candidate before it becomes a persisted
/// `ResourceLocator::Http`. This mirrors the controlled stream gate: the M3U
/// provider may only create HTTP(S) streams with an unambiguous authority.
fn validate_m3u_stream_url(url: &str) -> Result<(), AppError> {
    parse_http_url(url, HttpUrlPolicy::MediaResource)
        .map(|_| ())
        .map_err(|_| invalid_m3u_candidate())
}

/// Candidate IDs use percent-encoding so a remote URL or title never travels
/// as a directly callable URL through the UI.  Only the import service decodes
/// the opaque component after the source allowlist has been checked.
fn decode_candidate_component(value: &str) -> Result<String, AppError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "正文候选标识非法",
                false,
            ));
        }
        let high = hex_value(bytes[index + 1]).ok_or_else(|| {
            AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "正文候选标识非法",
                false,
            )
        })?;
        let low = hex_value(bytes[index + 2]).ok_or_else(|| {
            AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "正文候选标识非法",
                false,
            )
        })?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    let value = String::from_utf8(decoded).map_err(|_| {
        AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "正文候选标识非法",
            false,
        )
    })?;
    if value.is_empty() || has_control_character(&value) {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "正文候选标识非法",
            false,
        ));
    }
    Ok(value)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn invalid_m3u_candidate() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "M3U 候选标识非法",
        false,
    )
}

/// 在来源详情请求前校验 opaque candidate 解码出的远端身份。
///
/// 这不是 Provider 的网络校验替代品，而是 Application 边界的第一道
/// allowlist：前端只能通过候选句柄引用已经支持的标识形状，不能把 URL、
/// Header 片段、路径逃逸或任意长文本伪装成 remote_id 交给 Provider。
fn validate_remote_candidate_id(source_key: &str, value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > 512 || has_control_character(value) {
        return Err(invalid_remote_candidate_id());
    }

    let valid = match source_key {
        // MangaDex 作品身份是 UUID；章节身份由详情 Provider 在内部拼接，
        // 不允许候选阶段携带冒号或 CDN 地址。
        "mangadex" => is_canonical_uuid(value),
        // arXiv 的旧分类 ID 允许一个路径分隔符（例如 hep-th/9901001），
        // 但不能包含 URL、查询串、空路径段或目录逃逸。
        "arxiv" => {
            !value.starts_with('/')
                && !value.ends_with('/')
                && !value.contains("//")
                && !value.contains("://")
                && value.split('/').all(|part| {
                    !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
                })
        }
        // Europe PMC 只接受开放获取文章的 PMCID，不接受 PMID、URL 或任意查询。
        "europepmc" => {
            value.len() >= 4
                && value.len() <= 15
                && value.strip_prefix("PMC").is_some_and(|rest| {
                    !rest.is_empty()
                        && rest.len() <= 12
                        && rest.bytes().all(|byte| byte.is_ascii_digit())
                })
        }
        // Wikisource 标题可以包含命名空间冒号和斜杠，但不能伪装成 URL。
        "wikisource" => {
            !value.contains("://") && !value.contains(['\r', '\n']) && !value.trim().is_empty()
        }
        _ => false,
    };

    if valid {
        Ok(())
    } else {
        Err(invalid_remote_candidate_id())
    }
}

/// 校验已经持久化到 `ResourceLocator::SourceObject` 的完整远端身份。
///
/// 搜索候选和资源身份不是同一个形状：MangaDex 候选只有 manga UUID，
/// 而资源身份必须是 `manga_uuid:chapter_uuid`。下载、Session 和能力投影
/// 都必须调用这个 Application 级校验，不能各自用宽松的字符串判断。
/// 返回的错误不包含 remote_id，避免把来源身份或 URL 反射到用户文案/日志。
pub fn validate_remote_source_object(
    source_key: &str,
    resource_type: ResourceType,
    remote_id: &str,
) -> Result<(), AppError> {
    if remote_id.is_empty() || remote_id.len() > 512 || has_control_character(remote_id) {
        return Err(invalid_remote_candidate_id());
    }

    let expected_type = match source_key {
        "mangadex" => ResourceType::ComicArchive,
        "arxiv" | "opds_gutenberg" => ResourceType::PublicationFile,
        "europepmc" | "wikisource" => ResourceType::ArticleSnapshot,
        _ => return Err(invalid_remote_candidate_id()),
    };
    if resource_type != expected_type {
        return Err(AppError::new(
            "DOWNLOAD_SOURCE_UNSUPPORTED",
            ErrorKind::Validation,
            "远端资源类型不受支持",
            false,
        ));
    }

    let valid = match source_key {
        "mangadex" => {
            let Some((manga_id, chapter_id)) = remote_id.split_once(':') else {
                return Err(invalid_remote_candidate_id());
            };
            is_canonical_uuid(manga_id) && is_canonical_uuid(chapter_id)
        }
        "arxiv" => validate_arxiv_remote_id(remote_id),
        "europepmc" => remote_id.strip_prefix("PMC").is_some_and(|rest| {
            !rest.is_empty() && rest.len() <= 12 && rest.bytes().all(|byte| byte.is_ascii_digit())
        }),
        "wikisource" => {
            !remote_id.trim().is_empty()
                && remote_id.len() <= 300
                && !remote_id.contains("://")
                && !remote_id.chars().any(char::is_control)
        }
        "opds_gutenberg" => validate_gutenberg_remote_id(remote_id),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_remote_candidate_id())
    }
}

/// Provider payload MIME gate shared by capability projection and DownloadTask
/// creation.  The value is only a hint until the provider validates magic bytes,
/// but an absent or cross-provider hint must never be advertised as downloadable
/// in the first place.
pub fn remote_source_mime_compatible(
    source_key: &str,
    resource_type: ResourceType,
    mime_type: Option<&str>,
) -> bool {
    let Some(mime) = mime_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let expected = match source_key {
        "mangadex" if resource_type == ResourceType::ComicArchive => [
            "application/vnd.comicbook+zip",
            "application/zip",
            "application/x-cbz",
        ]
        .as_slice(),
        "arxiv" if resource_type == ResourceType::PublicationFile => ["application/pdf"].as_slice(),
        "opds_gutenberg" if resource_type == ResourceType::PublicationFile => {
            ["application/epub+zip"].as_slice()
        }
        "europepmc" | "wikisource" if resource_type == ResourceType::ArticleSnapshot => {
            ["text/html", "application/xhtml+xml"].as_slice()
        }
        _ => return false,
    };
    expected
        .iter()
        .any(|value| mime.eq_ignore_ascii_case(value))
}

fn is_canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|parsed| parsed.to_string() == value)
}

fn validate_arxiv_remote_id(value: &str) -> bool {
    value.len() <= 128
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains("://")
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        })
}

fn validate_gutenberg_remote_id(value: &str) -> bool {
    let Some(path) = value
        .strip_prefix("https://www.gutenberg.org/ebooks/")
        .or_else(|| value.strip_prefix("https://m.gutenberg.org/ebooks/"))
    else {
        return false;
    };
    !path.is_empty()
        && !path.contains("://")
        && !path.contains(['\\', '@', '#'])
        && path
            .split(['/', '?'])
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn invalid_remote_candidate_id() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "正文候选标识非法",
        false,
    )
}

#[allow(dead_code)]
fn validate_local_object_path(path: &str) -> Result<(), AppError> {
    if path.is_empty()
        || path.contains('\\')
        || std::path::Path::new(path).is_absolute()
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(AppError::new(
            "SOURCE_UNAVAILABLE",
            ErrorKind::Storage,
            "来源文件路径校验失败",
            true,
        ));
    }
    Ok(())
}

#[allow(dead_code)]
fn local_resource_type(media_type: MediaType, mime: &str) -> ResourceType {
    match media_type {
        MediaType::Comic => ResourceType::ComicArchive,
        MediaType::Article if mime.to_ascii_lowercase().contains("pdf") => {
            ResourceType::PublicationFile
        }
        MediaType::Article => ResourceType::ArticleSnapshot,
        MediaType::Book if mime.to_ascii_lowercase().contains("epub") => {
            ResourceType::PublicationFile
        }
        _ => ResourceType::LocalFile,
    }
}

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

/// 将公开来源 key 映射为稳定的内部 SourceId。
///
/// SourceId 是 UUID newtype，不能把来源注册表中的字符串直接塞进资源表，
/// 也不能在每次导入时随机生成 UUID，否则同一来源的资源无法稳定授权和去重。
/// 未列入 allowlist 的来源明确拒绝，避免把用户输入当成来源身份。
pub fn stable_source_id(source_key: &str) -> Result<SourceId, AppError> {
    let uuid = match source_key {
        "mangadex" => "9fd4a7d0-0d59-4dc5-9f96-5b9bca4c1001",
        "arxiv" => "9fd4a7d0-0d59-4dc5-9f96-5b9bca4c1002",
        "europepmc" => "9fd4a7d0-0d59-4dc5-9f96-5b9bca4c1003",
        "wikisource" => "9fd4a7d0-0d59-4dc5-9f96-5b9bca4c1004",
        "opds_gutenberg" => "9fd4a7d0-0d59-4dc5-9f96-5b9bca4c1005",
        _ => {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知正文来源",
                false,
            ));
        }
    };
    let uuid = Uuid::parse_str(uuid).map_err(|_| {
        AppError::new(
            "INTERNAL_ERROR",
            ErrorKind::Internal,
            "内置来源身份配置无效",
            false,
        )
    })?;
    Ok(SourceId::from_uuid(uuid))
}

/// 供 Download/Session 基础设施在消费 `SourceObject` 时把内部 UUID 映射回
/// 固定 Provider key。未知 UUID 不猜测、不回退到 URL。
pub fn source_key_for_id(source_id: SourceId) -> Option<&'static str> {
    [
        "mangadex",
        "arxiv",
        "europepmc",
        "wikisource",
        "opds_gutenberg",
    ]
    .into_iter()
    .find(|key| stable_source_id(key).ok() == Some(source_id))
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

#[cfg(test)]
mod candidate_tests {
    use super::*;

    #[test]
    fn opaque_candidate_component_roundtrips_url_without_control_bytes() {
        let encoded = "https%3A%2F%2Fwww.gutenberg.org%2Febooks%2F84.opds%3Ffoo%3Dbar%26lang%3Dzh";
        let decoded = decode_candidate_component(encoded).expect("encoded candidate is valid");
        assert_eq!(
            decoded,
            "https://www.gutenberg.org/ebooks/84.opds?foo=bar&lang=zh"
        );
        assert!(!encoded.contains('\u{1}'));
    }

    #[test]
    fn malformed_or_control_candidate_components_are_rejected() {
        for value in ["%", "%GG", "%C3%28", "abc%00def", "abc%01def"] {
            assert!(
                decode_candidate_component(value).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn remote_source_object_requires_canonical_mangadex_uuids() {
        let manga = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let chapter = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        assert!(
            validate_remote_source_object(
                "mangadex",
                ResourceType::ComicArchive,
                &format!("{manga}:{chapter}"),
            )
            .is_ok()
        );
        assert!(
            validate_remote_source_object(
                "mangadex",
                ResourceType::ComicArchive,
                &format!("{}:{chapter}", manga.to_ascii_uppercase()),
            )
            .is_err()
        );
        assert!(
            validate_remote_source_object(
                "mangadex",
                ResourceType::ComicArchive,
                &format!("{manga}:{{{chapter}}}"),
            )
            .is_err()
        );
        assert!(
            validate_remote_source_object(
                "mangadex",
                ResourceType::ComicArchive,
                &format!("{manga}:not-a-uuid"),
            )
            .is_err()
        );
    }

    #[test]
    fn remote_source_object_rejects_mismatched_types_and_urls() {
        assert!(
            validate_remote_source_object("arxiv", ResourceType::PublicationFile, "2401.12345",)
                .is_ok()
        );
        assert!(
            validate_remote_source_object("arxiv", ResourceType::ArticleSnapshot, "2401.12345",)
                .is_err()
        );
        assert!(
            validate_remote_source_object(
                "arxiv",
                ResourceType::PublicationFile,
                "https://evil.invalid/paper.pdf",
            )
            .is_err()
        );
        assert!(
            validate_remote_source_object(
                "opds_gutenberg",
                ResourceType::PublicationFile,
                "https://www.gutenberg.org/ebooks/84.epub3.images",
            )
            .is_ok()
        );
        assert!(
            validate_remote_source_object(
                "opds_gutenberg",
                ResourceType::PublicationFile,
                "https://evil.invalid/ebooks/84.epub3.images",
            )
            .is_err()
        );
        assert!(
            validate_remote_source_object(
                "opds_gutenberg",
                ResourceType::PublicationFile,
                "https://m.gutenberg.org/ebooks/84.opds",
            )
            .is_ok()
        );
    }

    #[test]
    fn stream_classification_uses_case_insensitive_path_extension() {
        assert_eq!(
            stream_url_kind("https://cdn.example/live/index.m3u8?token=opaque"),
            StreamUrlKind::Hls
        );
        assert_eq!(
            stream_url_kind("https://cdn.example/live/index.M3U"),
            StreamUrlKind::Hls
        );
        assert_eq!(
            stream_url_kind("https://cdn.example/live/manifest.MpD#fragment"),
            StreamUrlKind::Dash
        );
        assert_eq!(
            stream_mime("https://cdn.example/live/index.m3u8?format=mp4"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(
            stream_mime("https://cdn.example/live/manifest.mpd?token=.m3u8"),
            "application/dash+xml"
        );
    }

    #[test]
    fn stream_classification_ignores_query_and_requires_path_boundary() {
        for url in [
            "https://cdn.example/video.mp4?manifest=.m3u8",
            "https://cdn.example/video.mp4#manifest=.mpd",
            "https://cdn.example/video.m3u8.segment",
            "https://cdn.example/video.m3u8/segment",
            "https://cdn.example?path=/video.m3u8",
            "https://cdn.example/video.m3u8%3Ftoken=opaque",
        ] {
            assert_eq!(
                stream_url_kind(url),
                StreamUrlKind::Video,
                "query or an incomplete path suffix must not change type: {url}"
            );
        }
    }

    #[test]
    fn m3u_candidate_payload_allows_only_the_internal_separator() {
        let encoded = "%E6%96%B0%E9%97%BB%01https%3A%2F%2Fcdn.example%2Flive.m3u8%3Ftoken%3Dopaque";
        assert_eq!(
            decode_candidate_component_with_separator(encoded).unwrap(),
            "新闻\u{1}https://cdn.example/live.m3u8?token=opaque"
        );
        for value in ["title%00url", "title%02url", "%GG", "%C3%28"] {
            assert!(
                decode_candidate_component_with_separator(value).is_err(),
                "unsafe M3U candidate payload accepted: {value}"
            );
        }
    }

    #[test]
    fn m3u_stream_url_validation_is_fail_closed() {
        for url in [
            "https://cdn.example/live.m3u8",
            "http://cdn.example:8080/live.m3u?token=opaque",
            "https://[2001:4860:4860::8888]:8443/live.mp4",
        ] {
            assert!(
                validate_m3u_stream_url(url).is_ok(),
                "valid URL rejected: {url}"
            );
        }
        for url in [
            "",
            "ftp://cdn.example/live.m3u8",
            "https:///live.m3u8",
            "https://cdn.example:/live.m3u8",
            "https://cdn.example:99999/live.m3u8",
            "https://user:secret@cdn.example/live.m3u8",
            "https://cdn.example/live.m3u8 extra",
            "https://cdn.example/live\n.m3u8",
            "https://2001:db8::10/live.m3u8",
        ] {
            let err = validate_m3u_stream_url(url).unwrap_err();
            assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
        }
    }
}
