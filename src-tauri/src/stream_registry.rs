//! StreamRegistry：远端流播放授权（V2-B 实战批次；契约 §36.4）。
//!
//! - grant 是不透明 UUID，绑定 owner 窗口标签与上游主机白名单。
//! - 白名单初始只含资源自身 host；HLS manifest 经代理时把其中出现的主机
//!   收敛进白名单（防 SSRF：代理不会打开 grant 未学习到的任意地址）。
//! - 原始 URL 永不出本注册表。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use uuid::Uuid;

use haven_application::wire::ProgressSummaryDto;

/// 单个流会话的服务端事实快照。
#[derive(Debug, Clone)]
pub struct StreamGrantFacts {
    pub work_id: String,
    pub edition_id: String,
    pub media_item_id: String,
    pub mime_type: Option<String>,
    pub is_hls: bool,
    pub progress: Option<ProgressSummaryDto>,
    /// 初始上游地址（manifest 或直连媒体）；仅存于服务端。
    pub upstream_url: String,
}

pub struct StreamGrantInner {
    pub facts: StreamGrantFacts,
    pub owner_label: String,
    initial_host: String,
    allowed_hosts: Mutex<HashSet<String>>,
    created_at: Instant,
}

impl StreamGrantInner {
    /// 目标主机是否被授权（初始主机或 manifest 学习到的主机）。
    pub fn host_allowed(&self, host: &str) -> bool {
        if host == self.initial_host {
            return true;
        }
        self.allowed_hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(host)
    }

    /// 学习（收敛）manifest 中出现的主机。
    pub fn learn_hosts(&self, hosts: impl IntoIterator<Item = impl Into<String>>) {
        let mut allowed = self.allowed_hosts.lock().unwrap_or_else(|e| e.into_inner());
        allowed.extend(hosts.into_iter().map(Into::into));
    }
}

/// 流授权注册表。容量上限淘汰最旧，防无界增长。
#[derive(Default)]
pub struct StreamRegistry {
    grants: Mutex<std::collections::HashMap<Uuid, Arc<StreamGrantInner>>>,
}

const REGISTRY_CAP: usize = 32;

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册新 grant；返回不透明 UUID。
    pub fn register(&self, facts: StreamGrantFacts, upstream_url: &str, owner_label: &str) -> Uuid {
        let grant = Uuid::new_v4();
        let initial_host = host_of(upstream_url).unwrap_or_default();
        let mut hosts = HashSet::new();
        if !initial_host.is_empty() {
            hosts.insert(initial_host.clone());
        }
        let inner = Arc::new(StreamGrantInner {
            facts,
            owner_label: owner_label.to_owned(),
            initial_host,
            allowed_hosts: Mutex::new(hosts),
            created_at: Instant::now(),
        });
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        while grants.len() >= REGISTRY_CAP {
            if let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, g)| g.created_at)
                .map(|(k, _)| *k)
            {
                grants.remove(&oldest);
            } else {
                break;
            }
        }
        grants.insert(grant, inner);
        grant
    }

    /// 按 owner 校验后返回 grant（撤销/跨窗口访问一律 None）。
    pub fn lookup(&self, grant: &str, owner_label: &str) -> Option<Arc<StreamGrantInner>> {
        let id = Uuid::parse_str(grant).ok()?;
        let grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants
            .get(&id)
            .filter(|g| g.owner_label == owner_label)
            .cloned()
    }

    /// 撤销（幂等；返回是否存在过）。
    pub fn revoke(&self, grant: &str, owner_label: &str) -> bool {
        let Ok(id) = Uuid::parse_str(grant) else {
            return false;
        };
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants
            .get(&id)
            .is_some_and(|g| g.owner_label == owner_label)
            && grants.remove(&id).is_some()
    }
}

/// 从 URL 提取小写 host。
pub(crate) fn host_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> StreamGrantFacts {
        StreamGrantFacts {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            mime_type: Some("application/vnd.apple.mpegurl".into()),
            is_hls: true,
            progress: None,
            upstream_url: "https://cdn.example.com/a/index.m3u8".into(),
        }
    }

    #[test]
    fn register_lookup_revoke_by_owner() {
        let registry = StreamRegistry::new();
        let grant = registry.register(facts(), "https://cdn.example.com/a/index.m3u8", "main");
        let id = grant.to_string();
        assert!(registry.lookup(&id, "main").is_some());
        assert!(registry.lookup(&id, "other").is_none(), "跨窗口拒绝");
        assert!(registry.revoke(&id, "main"));
        assert!(!registry.revoke(&id, "main"), "幂等撤销");
        assert!(registry.lookup(&id, "main").is_none());
    }

    #[test]
    fn hosts_learned_via_manifest_are_authorized_others_denied() {
        let registry = StreamRegistry::new();
        let grant = registry.register(facts(), "https://cdn.example.com/a.m3u8", "main");
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        assert!(inner.host_allowed("cdn.example.com"));
        assert!(
            !inner.host_allowed("seg.other-cdn.net"),
            "未学习主机默认拒绝"
        );
        inner.learn_hosts(["seg.other-cdn.net"]);
        assert!(inner.host_allowed("seg.other-cdn.net"));
    }

    #[test]
    fn host_of_extracts_lowercased_host() {
        assert_eq!(
            host_of("HTTPS://CDN.Example.COM:8443/a/x.ts").as_deref(),
            Some("cdn.example.com")
        );
        assert_eq!(host_of("ftp://x/y"), None);
        assert_eq!(host_of("not-a-url"), None);
    }
}
