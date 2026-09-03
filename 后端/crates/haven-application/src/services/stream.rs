//! StreamService：远端流播放会话准备（V2-B 实战批次；契约 §36.4 受控代理 URI）。
//!
//! 与本地 Session（§17）并行的轻量通道：解析 MediaItem 的 Http 资源并返回
//! 服务端事实（upstream URL 等）；grant 注册与撤销由 src-tauri registry 完成。
//! 原始 URL 不出 IPC——前端只拿 `haven-resource://stream/<grant>` 代理 URI。

use std::sync::Arc;

use haven_common::network::{HttpUrlPolicy, parse_http_url};
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
use crate::services::source_import::{StreamUrlKind, stream_url_kind};
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

    /// 解析远端流资源。仅 Playback 引擎 + Http 定位 + 已接入的流类资源类型可开。
    /// DASH 保留为领域枚举，但当前 WebView 播放链路未完成，必须 fail-closed。
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
                // Keep the selection fail-closed. `ResourceService` exposes the
                // same narrow capability only for a valid HTTP(S) authority;
                // accepting a malformed URL here would make the detail page say
                // "not playable" while `stream_open` still registered a grant.
                matches!(&resource.locator, ResourceLocator::Http { url } if http_stream_online_readable(resource.resource_type, url))
                    && matches!(
                        resource.resource_type,
                        ResourceType::VideoStream | ResourceType::HlsStream
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
            // Keep the transport fact aligned with M3U import and the Tauri
            // proxy: classify only the URL path, case-insensitively. Query
            // parameters such as `?format=.m3u8` must not turn a direct
            // media URL into a manifest; an explicit HlsStream resource type
            // still wins when its provider uses a non-standard path.
            is_hls: is_hls_stream(resource.resource_type, url),
            mime_type: resource.mime_type.clone(),
            progress,
        })
    }
}

fn not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn is_hls_stream(resource_type: ResourceType, url: &str) -> bool {
    matches!(stream_url_kind(url), StreamUrlKind::Hls) || resource_type == ResourceType::HlsStream
}

/// Keep stream-open URL validation in lockstep with the capability projection
/// in `services::resource`. A stream grant must never be registered for an
/// arbitrary locator merely because its resource type looks like a stream.
///
/// This uses the shared outbound URL policy for scheme, authority, user-info,
/// literal private addresses, fragment and port validation. Redirects and
/// DNS pinning remain owned by the Tauri resource protocol after the grant is
/// created; this is the application-side fail-closed gate that prevents an
/// invalid locator from reaching that layer.
fn http_stream_online_readable(resource_type: ResourceType, raw_url: &str) -> bool {
    matches!(
        resource_type,
        ResourceType::VideoStream | ResourceType::HlsStream
    ) && parse_http_url(raw_url, HttpUrlPolicy::MediaResource).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_open_accepts_only_supported_http_stream_locators() {
        for resource_type in [ResourceType::VideoStream, ResourceType::HlsStream] {
            for url in [
                "https://stream.example.test/video/index.m3u8?token=opaque",
                "http://stream.example.test/video/file.mp4",
                "https://stream.example.test:8443/video/index.m3u8",
                "https://[2001:4860:4860::8888]:8443/video/index.m3u8",
            ] {
                assert!(
                    http_stream_online_readable(resource_type, url),
                    "valid stream locator rejected: {url}"
                );
            }
        }

        assert!(!http_stream_online_readable(
            ResourceType::DashStream,
            "https://stream.example.test/video/manifest.mpd"
        ));

        for resource_type in [
            ResourceType::HttpFile,
            ResourceType::PublicationFile,
            ResourceType::ArticleSnapshot,
        ] {
            assert!(!http_stream_online_readable(
                resource_type,
                "https://stream.example.test/video/file.mp4"
            ));
        }
    }

    #[test]
    fn stream_open_rejects_malformed_or_ambiguous_locators() {
        for url in [
            "",
            "ftp://stream.example.test/video.m3u8",
            "file:///tmp/video.m3u8",
            "https:///video.m3u8",
            "https://",
            "https://stream.example.test: /video.m3u8",
            "https://stream.example.test:99999/video.m3u8",
            "https://stream.example.test:/video.m3u8",
            "https://[2001:db8::10/video.m3u8",
            "https://2001:db8::10/video.m3u8",
            "https://user:secret@stream.example.test/video.m3u8",
            "https://stream.example.test/video\n.m3u8",
            "https://stream.example.test/video.m3u8 extra",
        ] {
            assert!(
                !http_stream_online_readable(ResourceType::HlsStream, url),
                "malformed stream locator was accepted: {url:?}"
            );
        }
    }

    #[test]
    fn stream_open_hls_fact_ignores_query_extension_and_honors_path_case() {
        assert!(!is_hls_stream(
            ResourceType::VideoStream,
            "https://stream.example.test/video.mp4?format=.m3u8"
        ));
        assert!(is_hls_stream(
            ResourceType::VideoStream,
            "https://stream.example.test/live/INDEX.M3U8?token=opaque"
        ));
        assert!(is_hls_stream(
            ResourceType::HlsStream,
            "https://stream.example.test/live/channel"
        ));
    }
}
