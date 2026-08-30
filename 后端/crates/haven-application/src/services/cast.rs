//! CastService：投屏编排（v02-cast-001 双栈）。
//! - 发现：透传 Port（DLNA SSDP + Chromecast mDNS 合并）。
//! - 播放：复用 StreamService::prepare 解析 upstream_url → 注册 CastGrant → 控制面推流。
//! - 状态/停止：透传 Port，幂等。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind};

use crate::services::stream::StreamService;
use crate::wire::{
    CastDeviceDto, CastDiscoverRequest, CastDiscoverResult, CastPlayRequest, CastPlayResult,
    CastStatusDto, CastStatusRequest, CastStopRequest, CastStopResult, CastTransportStateDto,
    SessionEngineDto,
};

#[async_trait]
pub trait CastDiscoveryPort: Send + Sync {
    async fn discover(&self, timeout_ms: u32) -> Result<Vec<CastDeviceDto>, AppError>;
}

#[async_trait]
pub trait CastControlPort: Send + Sync {
    async fn play(&self, device_id: &str, lan_url: &str, title: &str) -> Result<(), AppError>;
    async fn stop(&self, device_id: &str) -> Result<(), AppError>;
    async fn status(&self, device_id: &str) -> Result<CastTransportStateDto, AppError>;
    async fn position(&self, device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError>;
}

#[async_trait]
pub trait CastMediaPort: Send + Sync {
    fn register(&self, grant: String, upstream_url: String, title: String) -> String;
    fn revoke(&self, grant: &str) -> bool;
    fn lan_url(&self, grant: &str) -> Option<String>;
}

#[derive(Clone)]
pub struct CastGrantRegistry {
    inner: Arc<Mutex<HashMap<String, CastGrant>>>,
    base_url: String,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct CastGrant {
    grant: String,
    upstream_url: String,
    device_id: String,
}

impl CastGrantRegistry {
    pub fn new(base_url: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            base_url,
        }
    }

    pub fn register(&self, grant: String, upstream_url: String, device_id: String) -> String {
        let lan = format!("{}/cast/{}", self.base_url.trim_end_matches('/'), grant);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(
            grant.clone(),
            CastGrant {
                grant,
                upstream_url,
                device_id,
            },
        );
        lan
    }

    pub fn revoke(&self, grant: &str) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(grant).is_some()
    }

    pub fn lan_url(&self, grant: &str) -> Option<String> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.get(grant)
            .map(|v| format!("{}/cast/{}", self.base_url.trim_end_matches('/'), v.grant))
    }
}

#[derive(Clone)]
pub struct CastService {
    discovery: Arc<dyn CastDiscoveryPort>,
    control: Arc<dyn CastControlPort>,
    media: Arc<dyn CastMediaPort>,
    stream: StreamService,
    grant_registry: Arc<CastGrantRegistry>,
}

impl CastService {
    pub fn new(
        discovery: Arc<dyn CastDiscoveryPort>,
        control: Arc<dyn CastControlPort>,
        media: Arc<dyn CastMediaPort>,
        stream: StreamService,
        grant_registry: Arc<CastGrantRegistry>,
    ) -> Self {
        Self {
            discovery,
            control,
            media,
            stream,
            grant_registry,
        }
    }

    pub async fn discover(&self, req: CastDiscoverRequest) -> Result<CastDiscoverResult, AppError> {
        let timeout_ms = req.timeout_ms.unwrap_or(5000);
        if timeout_ms == 0 || timeout_ms > 10_000 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "timeout_ms 需在 1..10000",
                false,
            ));
        }
        let devices = self.discovery.discover(timeout_ms).await?;
        Ok(CastDiscoverResult {
            schema_version: 1,
            devices,
        })
    }

    pub async fn play(&self, req: CastPlayRequest) -> Result<CastPlayResult, AppError> {
        if req.media_item_id.trim().is_empty() || req.device_id.trim().is_empty() {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "参数缺失",
                false,
            ));
        }
        if req.engine != SessionEngineDto::Playback {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "仅支持 playback 引擎投屏",
                false,
            ));
        }
        // 复用 StreamService 校验与上游解析（远端流条目校验 is_hls 等）。
        let facts = self
            .stream
            .prepare(crate::wire::SessionOpenRequest {
                media_item_id: req.media_item_id.clone(),
                engine: req.engine,
            })
            .await?;
        let grant = uuid::Uuid::new_v4().to_string();
        let title = facts.media_item_id.clone();
        // A 路线：远端 HLS 直通上游（无需 LAN 代理；本地文件批次再切回 media.register 的 /cast 代理）
        let lan_url = if facts.is_hls {
            // 编码中文路径后直通，TV 可直拉
            let encoded = encode_url_for_cast(&facts.upstream_url);
            self.media
                .register(grant.clone(), encoded.clone(), title.clone());
            self.grant_registry
                .register(grant.clone(), encoded.clone(), req.device_id.clone());
            encoded
        } else {
            let url = self
                .media
                .register(grant.clone(), facts.upstream_url.clone(), title.clone());
            self.grant_registry.register(
                grant.clone(),
                facts.upstream_url.clone(),
                req.device_id.clone(),
            );
            url
        };
        // 控制面推流（失败需回滚 grant）
        if let Err(e) = self.control.play(&req.device_id, &lan_url, &title).await {
            self.media.revoke(&grant);
            self.grant_registry.revoke(&grant);
            return Err(e);
        }
        Ok(CastPlayResult {
            schema_version: 1,
            cast_session_id: grant,
            lan_url,
            device_name: req.device_id.clone(),
        })
    }

    pub async fn status(&self, req: CastStatusRequest) -> Result<CastStatusDto, AppError> {
        // cast_session_id 即 grant；需从 registry 取 device_id
        let device_id = {
            let g = self
                .grant_registry
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            g.get(&req.cast_session_id)
                .map(|v| v.device_id.clone())
                .ok_or_else(|| {
                    AppError::new(
                        "RESOURCE_NOT_FOUND",
                        ErrorKind::NotFound,
                        "投屏会话不存在或已结束",
                        false,
                    )
                })?
        };
        let transport_state = self
            .control
            .status(&device_id)
            .await
            .unwrap_or(CastTransportStateDto::Unknown);
        let (position_ms, duration_ms) = self
            .control
            .position(&device_id)
            .await
            .unwrap_or((None, None));
        Ok(CastStatusDto {
            schema_version: 1,
            transport_state,
            position_ms,
            duration_ms,
        })
    }

    pub async fn stop(&self, req: CastStopRequest) -> Result<CastStopResult, AppError> {
        let device_id = {
            let mut g = self
                .grant_registry
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            g.remove(&req.cast_session_id).map(|v| v.device_id)
        };
        if let Some(did) = device_id {
            let _ = self.control.stop(&did).await;
        }
        self.media.revoke(&req.cast_session_id);
        Ok(CastStopResult {
            schema_version: 1,
            stopped: true,
        })
    }
}

fn encode_url_for_cast(url: &str) -> String {
    let mut out = String::new();
    let mut chars = url.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&h) = chars.peek() {
                if h.is_ascii_hexdigit() {
                    chars.next();
                    if let Some(lo) = chars.next() {
                        if lo.is_ascii_hexdigit() {
                            out.push('%');
                            out.push(h);
                            out.push(lo);
                            continue;
                        }
                    }
                }
            }
            out.push_str(&format!("%{:02X}", c as u8));
        } else if c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '_'
                    | '.'
                    | '~'
                    | ':'
                    | '/'
                    | '?'
                    | '='
                    | '&'
                    | '#'
                    | '@'
                    | '!'
                    | '$'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
            )
        {
            out.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

// 仅用于测试的内存实现
#[cfg(test)]
pub mod test_doubles {
    use crate::wire::CastProtocolDto;

    use super::*;

    pub struct NoopDiscovery;
    #[async_trait]
    impl CastDiscoveryPort for NoopDiscovery {
        async fn discover(&self, _timeout_ms: u32) -> Result<Vec<CastDeviceDto>, AppError> {
            Ok(vec![CastDeviceDto {
                device_id: "test-dlna-1".into(),
                friendly_name: "测试电视".into(),
                ip: "192.168.1.100".into(),
                protocol: CastProtocolDto::Dlna,
                model_name: Some("TestTV".into()),
            }])
        }
    }

    pub struct NoopControl;
    #[async_trait]
    impl CastControlPort for NoopControl {
        async fn play(
            &self,
            _device_id: &str,
            _lan_url: &str,
            _title: &str,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn stop(&self, _device_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn status(&self, _device_id: &str) -> Result<CastTransportStateDto, AppError> {
            Ok(CastTransportStateDto::Playing)
        }
        async fn position(&self, _device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError> {
            Ok((Some(5000), Some(1526000)))
        }
    }

    pub struct NoopMedia;
    #[async_trait]
    impl CastMediaPort for NoopMedia {
        fn register(&self, grant: String, _upstream_url: String, _title: String) -> String {
            format!("http://127.0.0.1:3500/cast/{}", grant)
        }
        fn revoke(&self, _grant: &str) -> bool {
            true
        }
        fn lan_url(&self, grant: &str) -> Option<String> {
            Some(format!("http://127.0.0.1:3500/cast/{}", grant))
        }
    }
}
