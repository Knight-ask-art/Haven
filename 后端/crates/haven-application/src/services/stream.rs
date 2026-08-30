//! StreamService：远端流播放会话准备（V2-B 实战批次；契约 §36.4 受控代理 URI）。
//!
//! 与本地 Session（§17）并行的轻量通道：解析 MediaItem 的 Http 资源并返回
//! 服务端事实（upstream URL 等）；grant 注册与撤销由 src-tauri registry 完成。
//! 原始 URL 不出 IPC——前端只拿 `haven-resource://stream/<grant>` 代理 URI。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ProgressRepository, ResourceRepository, WorkRepository,
};
use haven_domain::entities::{Resource, ResourceLocator};
use haven_domain::enums::{Availability, ResourceType};
use haven_domain::ids::{MediaItemId, ResourceId};

use crate::mapper::progress::progress_summary;
use crate::services::ports::SessionOpenPorts;
use crate::services::session::engine_compatible;
use crate::wire::{ProgressSummaryDto, SessionEngineDto, SessionOpenRequest};

/// 流会话服务端事实（不出 IPC）。
#[derive(Debug, Clone)]
pub struct StreamOpenFacts {
    pub work_id: String,
    pub edition_id: String,
    pub media_item_id: String,
    pub resource_id: ResourceId,
    pub upstream_url: String,
    pub is_hls: bool,
    pub mime_type: Option<String>,
    pub progress: Option<ProgressSummaryDto>,
}

#[derive(Clone)]
pub struct StreamService {
    ports: Arc<dyn SessionOpenPorts>,
}

impl StreamService {
    pub fn new(ports: Arc<dyn SessionOpenPorts>) -> Self {
        Self { ports }
    }

    /// 解析远端流资源。仅 Playback 引擎 + Http 定位 + 流类资源类型可开。
    pub async fn prepare(&self, request: SessionOpenRequest) -> Result<StreamOpenFacts, AppError> {
        let media_item_id: MediaItemId = request.media_item_id.parse().map_err(|_| {
            AppError::new(
                "INVALID_ID",
                ErrorKind::Validation,
                "无效的媒体条目 ID",
                false,
            )
        })?;
        if request.engine != SessionEngineDto::Playback {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "流会话仅支持播放引擎",
                false,
            ));
        }

        let media_item = MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .ok_or_else(|| not_found("MEDIA_ITEM_NOT_FOUND", "媒体条目不存在"))?;
        if !engine_compatible(request.engine, media_item.media_type) {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前媒介格式不支持该引擎",
                false,
            ));
        }
        let edition = EditionRepository::get(&*self.ports, media_item.edition_id)
            .await?
            .ok_or_else(|| not_found("EDITION_NOT_FOUND", "版本不存在"))?;
        WorkRepository::get(&*self.ports, edition.work_id)
            .await?
            .ok_or_else(|| not_found("WORK_NOT_FOUND", "作品不存在"))?;

        let resources = ResourceRepository::list_by_media_item(&*self.ports, media_item_id).await?;
        let mut candidates: Vec<&Resource> = resources
            .iter()
            .filter(|resource| {
                matches!(resource.locator, ResourceLocator::Http { .. })
                    && matches!(
                        resource.resource_type,
                        ResourceType::VideoStream
                            | ResourceType::HlsStream
                            | ResourceType::DashStream
                    )
                    && matches!(
                        resource.availability,
                        Availability::Available | Availability::OfflineAvailable
                    )
            })
            .collect();
        candidates.sort_by_key(|resource| resource.id.to_string());
        let Some(resource) = candidates.first() else {
            return Err(not_found("RESOURCE_NOT_FOUND", "没有可用的远端流资源"));
        };
        let ResourceLocator::Http { url } = &resource.locator else {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                ErrorKind::Security,
                "资源定位不允许由流会话打开",
                false,
            ));
        };
        let progress = ProgressRepository::get_for_media_item(&*self.ports, media_item_id)
            .await?
            .as_ref()
            .map(progress_summary)
            .transpose()?;

        Ok(StreamOpenFacts {
            work_id: edition.work_id.to_string(),
            edition_id: edition.id.to_string(),
            media_item_id: media_item.id.to_string(),
            resource_id: resource.id,
            upstream_url: url.clone(),
            is_hls: url.contains(".m3u8") || resource.resource_type == ResourceType::HlsStream,
            mime_type: resource.mime_type.clone(),
            progress,
        })
    }
}

fn not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}
