//! LAN 媒体服务：动态端口 3500-4499（当前批仅本地文件投屏启用；远端 HLS 直通上游，无需本服务）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_application::services::CastMediaPort;

pub struct AxumCastMediaServer {
    inner: Arc<Mutex<HashMap<String, String>>>,
    base: String,
}

impl Default for AxumCastMediaServer {
    fn default() -> Self {
        Self::new()
    }
}

impl AxumCastMediaServer {
    pub fn new() -> Self {
        // 本批不启动 axum 监听（远端 HLS 直通）；保留端口探测与 base 计算供本地文件批次复用
        let base = "http://127.0.0.1:3500".to_owned();
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            base,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }
}

#[async_trait]
impl CastMediaPort for AxumCastMediaServer {
    fn register(&self, grant: String, upstream_url: String, _title: String) -> String {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        m.insert(grant.clone(), upstream_url.clone());
        // 远端 HLS 直通：直接返回上游（TV 可直拉），本地文件批次再切回 /cast/<grant> 代理
        // 为保持契约 lanUrl 仍为 http(s) 可达 URL，前端与 TV 均可直接使用
        upstream_url
    }

    fn revoke(&self, grant: &str) -> bool {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        m.remove(grant).is_some()
    }

    fn lan_url(&self, grant: &str) -> Option<String> {
        let m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        m.get(grant).cloned()
    }
}
